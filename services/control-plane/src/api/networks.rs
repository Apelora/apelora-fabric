// SPDX-License-Identifier: AGPL-3.0-only
//! The Console's Networks area (docs/development/CONSOLE-PLAN.md), read-only: the network
//! settings in effect (uplink, address ranges, the deploy projects' network limits) and, per
//! application, what Apelora intends — its OVN network, the ACL rules compiled from the
//! design's connections, service and published addresses — next to what Incus reports, with
//! the differences listed as drift.
//!
//! The intended rules are rebuilt from the deployment Apelora recorded (desired modules,
//! connections, published ports, uplink, service range) with the same function `network.sync`
//! uses (`SyncRequest::intended_acls`), so the page never shows rules Apelora would not write.

use std::collections::{BTreeMap, BTreeSet};

use apelora_auth::Resource;
use apelora_domain::network::{
    AclRule, Connection, Internet, Published, SyncRequest, ipv4_range_within, network_name,
    rule_drift, service_addresses,
};
use apelora_incus_adapter::{AdapterError, ObservedAcl, ObservedAddressSet, ObservedLoadBalancer};
use axum::extract::State;
use axum::{Extension, Json};
use serde::Serialize;
use utoipa::ToSchema;

use super::authz::{allow, allow_platform};
use super::deploy::store;
use super::manage::desired_of;
use super::{ApiError, AppState, ErrorBody, Principal, Source, source};

type Res<T> = Result<T, ApiError>;

// ---- settings in effect -------------------------------------------------------------------------

/// A deploy project's network limits, as Incus reports them.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NetProjectLimits {
    pub project: String,
    /// Incus reports the project (the deploy identity can read it).
    pub found: bool,
    pub restricted: bool,
    /// `features.networks`: the project has its own networks (needed for per-application OVN
    /// networks).
    pub own_networks: bool,
    /// `restricted.networks.uplinks`: uplinks its networks may use.
    pub uplinks: Vec<String>,
    /// `restricted.networks.subnets`: `<uplink>:<range>` its load balancers and forwards may
    /// listen on.
    pub subnets: Vec<String>,
}

/// The uplink network, as Incus reports it (read with the administrator identity).
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NetUplink {
    pub name: String,
    pub kind: String,
    pub status: String,
    pub ipv4_address: Option<String>,
    /// `ipv4.routes`: ranges routed to the uplink (service addresses must be in one).
    pub ipv4_routes: Vec<String>,
    /// `ipv4.ovn.ranges`: addresses OVN networks take on the uplink.
    pub ovn_ranges: Vec<String>,
}

/// Something about the settings worth knowing, from comparing them with Incus.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NetFinding {
    /// `warn`: Incus will refuse or traffic will not flow; `info`: a fact.
    pub severity: String,
    pub message: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NetSettings {
    pub source: Source,
    /// `APELORA_NETWORK_UPLINK`; `None`: connections are not enforced.
    pub uplink: Option<String>,
    /// `APELORA_SERVICE_ADDRESSES`; `None`: no service addresses.
    pub service_range: Option<String>,
    /// Service addresses held by applications.
    pub service_addresses_used: i64,
    /// Addresses the range holds; `None` without a range.
    pub service_addresses_total: Option<u32>,
    pub projects: Vec<NetProjectLimits>,
    pub uplink_observed: Option<NetUplink>,
    /// Why the uplink is not shown (identity, error).
    pub uplink_note: Option<String>,
    pub findings: Vec<NetFinding>,
}

fn list(config: &BTreeMap<String, String>, key: &str) -> Vec<String> {
    config
        .get(key)
        .map(|v| {
            v.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn warn(message: String) -> NetFinding {
    NetFinding {
        severity: "warn".into(),
        message,
    }
}

/// The network settings in effect: Apelora's uplink and service range, each deploy project's
/// network limits, and the uplink as Incus reports it (with the administrator identity, when
/// configured), with what does not fit together. Read-only. Needs `cluster.view`.
#[utoipa::path(get, path = "/api/v1/networks/settings", tag = "inventory", operation_id = "network_settings",
    responses((status = 200, body = NetSettings), (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody), (status = 502, body = ErrorBody)))]
pub(super) async fn settings(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Res<Json<NetSettings>> {
    allow_platform(&p, "cluster.view")?;
    let uplink = state.network_uplink.as_deref().map(str::to_owned);
    let service_range = state.service_range.as_deref().map(str::to_owned);
    let used = match &state.auth {
        super::Auth::Tokens(pool) => apelora_persistence::service_addresses_held(pool).await?,
        super::Auth::DisabledForDevelopment => 0,
    };
    let total = service_range
        .as_deref()
        .and_then(|r| service_addresses(r).ok())
        .map(|a| u32::try_from(a.len()).unwrap_or(u32::MAX));
    let mut findings = Vec::new();
    let mut projects = Vec::new();
    // Apelora's projects: the configured deploy projects and every environment's.
    let mut all: BTreeSet<String> = state.deploy_projects.iter().cloned().collect();
    if let super::Auth::Tokens(pool) = &state.auth {
        all.extend(apelora_persistence::identity::environment_incus_projects(pool).await?);
    }
    for project in &all {
        let config = state.provider.project_config(project).await?;
        let found = config.is_some();
        let config = config.unwrap_or_default();
        let limits = NetProjectLimits {
            project: project.clone(),
            found,
            restricted: config.get("restricted").is_some_and(|v| v == "true"),
            own_networks: config.get("features.networks").is_some_and(|v| v == "true"),
            uplinks: list(&config, "restricted.networks.uplinks"),
            subnets: list(&config, "restricted.networks.subnets"),
        };
        if !found {
            findings.push(warn(format!(
                "Project {project} does not exist in Incus or is not visible to the deploy identity"
            )));
        } else if let Some(u) = &uplink {
            if !limits.own_networks {
                findings.push(warn(format!(
                    "Project {project} has no networks of its own (features.networks): application networks cannot be created in it"
                )));
            }
            if limits.restricted && !limits.uplinks.iter().any(|x| x == u) {
                findings.push(warn(format!(
                    "Project {project} may not use uplink {u} (restricted.networks.uplinks): Incus refuses application networks"
                )));
            }
            if let Some(r) = &service_range
                && limits.restricted
                && !limits.subnets.iter().any(|s| {
                    s.split_once(':')
                        .is_some_and(|(net, range)| net == u && ipv4_range_within(r, range))
                })
            {
                findings.push(warn(format!(
                    "Service range {r} is not inside project {project}'s restricted.networks.subnets for {u}: Incus refuses its load balancers"
                )));
            }
        }
        projects.push(limits);
    }
    let (uplink_observed, uplink_note) = match (&uplink, &state.admin) {
        (None, _) => (None, None),
        (Some(_), None) => (
            None,
            Some("The deploy identity cannot read the uplink (it belongs to the default project); configure the administrator identity to show its routes and ranges.".to_owned()),
        ),
        (Some(u), Some(admin)) => match admin.network("default", u).await {
            Ok(Some(n)) => (
                Some(NetUplink {
                    ipv4_address: n.config.get("ipv4.address").cloned(),
                    ipv4_routes: list(&n.config, "ipv4.routes"),
                    ovn_ranges: list(&n.config, "ipv4.ovn.ranges"),
                    name: n.name,
                    kind: n.kind,
                    status: n.status,
                }),
                None,
            ),
            Ok(None) => {
                findings.push(warn(format!(
                    "Uplink {u} does not exist in Incus's default project"
                )));
                (None, Some(format!("Incus has no network {u}")))
            }
            Err(e) => (None, Some(format!("The uplink could not be read: {e}"))),
        },
    };
    if let (Some(r), Some(up)) = (&service_range, &uplink_observed)
        && !up.ipv4_routes.iter().any(|x| ipv4_range_within(r, x))
    {
        findings.push(warn(format!(
            "Service range {r} is not routed to uplink {} (ipv4.routes): outside clients cannot reach service addresses",
            up.name
        )));
    }
    if uplink.is_none() {
        findings.push(NetFinding {
            severity: "info".into(),
            message: "No uplink is set (APELORA_NETWORK_UPLINK): connections are not enforced and instances share their project's network".into(),
        });
    } else if service_range.is_none() {
        findings.push(NetFinding {
            severity: "info".into(),
            message: "No service range is set (APELORA_SERVICE_ADDRESSES): modules get no service or published addresses".into(),
        });
    }
    Ok(Json(NetSettings {
        source: source(state.provider.mode()),
        uplink,
        service_range,
        service_addresses_used: used,
        service_addresses_total: total,
        projects,
        uplink_observed,
        uplink_note,
        findings,
    }))
}

// ---- per application ----------------------------------------------------------------------------

/// One ACL rule, intended or found in Incus, with how the two compare.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NetRule {
    pub action: String,
    /// `$<address set>` (a module's instances), `@external` (outside), or empty (any).
    pub source: String,
    pub destination: String,
    pub protocol: String,
    /// Empty: any port.
    pub port: String,
    pub description: String,
    /// `applied` (intended and in Incus), `missing` (intended, not in Incus), `unexpected` (in
    /// Incus, not intended) or `unknown` (Incus could not be read).
    pub state: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NetModule {
    pub module: String,
    /// The module's ACL and address set.
    pub acl: String,
    pub internet: Internet,
    pub ingress: Vec<NetRule>,
    pub egress: Vec<NetRule>,
    /// The ACL exists in Incus; `None` when Incus could not be read.
    pub acl_present: Option<bool>,
    /// The module's instances' addresses (its address set); `None` when not read.
    pub addresses: Option<Vec<String>>,
    pub drift: Vec<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NetPort {
    pub transport: String,
    pub port: u16,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NetBackend {
    pub name: String,
    pub address: String,
    /// The instance behind it (from the backend's description).
    pub instance: String,
}

/// A module's service address: one load balancer over its replicas.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NetService {
    pub module: String,
    /// Allocated by Apelora; `None` while none is free (or before the first sync).
    pub address: Option<String>,
    pub ports: Vec<NetPort>,
    /// Reachable from outside the application.
    pub published: bool,
    /// The load balancer's backends in Incus; `None` when it is missing or Incus was not read.
    pub backends: Option<Vec<NetBackend>>,
    pub drift: Vec<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NetObservedNetwork {
    pub kind: String,
    pub status: String,
    pub ipv4_address: Option<String>,
    /// The network's address on the uplink (`volatile.network.ipv4.address`): outside traffic
    /// to published addresses is routed here.
    pub router_address: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NetApplication {
    pub application: String,
    pub project: String,
    /// The deployed revision these settings come from.
    pub revision: i32,
    /// Connections are enforced on the application's own OVN network.
    pub enforced: bool,
    pub network: String,
    pub uplink: Option<String>,
    pub service_range: Option<String>,
    /// `None`: missing in Incus, not enforced, or Incus could not be read (see `incusError`).
    pub network_observed: Option<NetObservedNetwork>,
    /// Which module may reach which, on what transport and port (from the design).
    pub connections: Vec<Connection>,
    pub published: Vec<Published>,
    pub modules: Vec<NetModule>,
    pub services: Vec<NetService>,
    /// Differences between what Apelora intends and what Incus holds, for the whole application.
    pub drift: Vec<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NetApplications {
    pub source: Source,
    /// Why Incus could not be read; the intended side is still shown.
    pub incus_error: Option<String>,
    pub items: Vec<NetApplication>,
}

fn net_rule(r: &AclRule, state: &str) -> NetRule {
    NetRule {
        action: r.action.clone(),
        source: r.source.clone(),
        destination: r.destination.clone(),
        protocol: r.protocol.clone(),
        port: r.destination_port.clone(),
        description: r.description.clone(),
        state: state.into(),
    }
}

/// Intended rules marked against what Incus holds; unexpected rules from Incus are appended.
fn compare(
    intended: &[AclRule],
    observed: Option<&[AclRule]>,
    side: &str,
    drift: &mut Vec<String>,
) -> Vec<NetRule> {
    let Some(observed) = observed else {
        return intended.iter().map(|r| net_rule(r, "unknown")).collect();
    };
    let (missing, extra) = rule_drift(intended, observed);
    let mut out: Vec<NetRule> = intended
        .iter()
        .map(|r| {
            net_rule(
                r,
                if missing.contains(r) {
                    "missing"
                } else {
                    "applied"
                },
            )
        })
        .collect();
    out.extend(extra.iter().map(|r| net_rule(r, "unexpected")));
    if !missing.is_empty() {
        drift.push(format!(
            "{} {side} rule{} not in Incus",
            missing.len(),
            if missing.len() == 1 { "" } else { "s" }
        ));
    }
    if !extra.is_empty() {
        drift.push(format!(
            "{} {side} rule{} in Incus that the design does not have",
            extra.len(),
            if extra.len() == 1 { "" } else { "s" }
        ));
    }
    out
}

/// What Incus holds for one project.
struct ProjectObserved {
    acls: Vec<ObservedAcl>,
    sets: Vec<ObservedAddressSet>,
}

/// Each application the caller may view that has a recorded deployment: its network, the
/// rules compiled from its design, its service and published addresses, and what Incus holds
/// for each, with the differences. Read-only. Needs `application.view` on each application.
#[utoipa::path(get, path = "/api/v1/networks/applications", tag = "applications", operation_id = "application_networks",
    responses((status = 200, body = NetApplications), (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody)))]
pub(super) async fn applications(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Res<Json<NetApplications>> {
    let pool = store(&state)?;
    let mut deployments = Vec::new();
    for d in apelora_persistence::list_deployments(pool).await? {
        let res = apelora_persistence::identity::application_context(pool, &d.application)
            .await?
            .map_or_else(Resource::platform, |c| c.resource());
        if allow(&p, "application.view", &res).is_ok() {
            deployments.push(d);
        }
    }
    // Incus once per project (ACLs and address sets) and once per application network.
    let mut incus_error: Option<String> = None;
    let mut by_project: BTreeMap<String, ProjectObserved> = BTreeMap::new();
    let projects: BTreeSet<String> = deployments
        .iter()
        .filter(|d| d.uplink.is_some())
        .map(|d| d.project.clone())
        .collect();
    let read: Result<(), AdapterError> = async {
        for project in projects {
            let acls = state.provider.network_acls(&project).await?;
            let sets = state.provider.address_sets(&project).await?;
            by_project.insert(project, ProjectObserved { acls, sets });
        }
        Ok(())
    }
    .await;
    if let Err(e) = read {
        tracing::warn!(error = %e, "networks: Incus could not be read");
        incus_error = Some(e.to_string());
    }
    let mut items = Vec::with_capacity(deployments.len());
    for d in deployments {
        let desired = desired_of(&d);
        let connections: Vec<Connection> = d
            .connections
            .clone()
            .and_then(|c| serde_json::from_value(c).ok())
            .unwrap_or_default();
        let published: Vec<Published> = d
            .published
            .clone()
            .and_then(|c| serde_json::from_value(c).ok())
            .unwrap_or_default();
        let network = network_name(&d.application);
        let sync = SyncRequest::for_deployment(
            &d.application,
            &d.project,
            d.uplink.as_deref().unwrap_or_default(),
            &desired,
            &connections,
            d.service_range.as_deref(),
        )
        .with_published(&published);
        let addresses: BTreeMap<String, String> =
            apelora_persistence::application_services(pool, &d.application)
                .await?
                .into_iter()
                .filter(|(project, _, _)| project == &d.project)
                .map(|(_, module, address)| (module, address))
                .collect();
        let mut drift = Vec::new();
        let enforced = d.uplink.is_some();
        let observed = if enforced && incus_error.is_none() {
            by_project.get(&d.project)
        } else {
            None
        };
        let (network_observed, lbs): (
            Option<NetObservedNetwork>,
            Option<Vec<ObservedLoadBalancer>>,
        ) = match (enforced, observed.is_some()) {
            (true, true) => {
                let n = state.provider.network(&d.project, &network).await;
                let l = state.provider.load_balancers(&d.project, &network).await;
                match (n, l) {
                    (Ok(n), Ok(l)) => {
                        if n.is_none() {
                            drift.push(format!("network {network} is not in Incus"));
                        }
                        (
                            n.map(|n| NetObservedNetwork {
                                ipv4_address: n.config.get("ipv4.address").cloned(),
                                router_address: n
                                    .config
                                    .get("volatile.network.ipv4.address")
                                    .cloned(),
                                kind: n.kind,
                                status: n.status,
                            }),
                            Some(l),
                        )
                    }
                    (Err(e), _) | (_, Err(e)) => {
                        incus_error.get_or_insert_with(|| e.to_string());
                        (None, None)
                    }
                }
            }
            _ => (None, None),
        };
        let modules: Vec<NetModule> = if enforced {
            sync.intended_acls()
                .into_iter()
                .map(|a| {
                    let mut mdrift = Vec::new();
                    let found = observed.map(|o| o.acls.iter().find(|x| x.name == a.acl));
                    let acl_present = found.map(|f| f.is_some());
                    if acl_present == Some(false) {
                        mdrift.push(format!("ACL {} is not in Incus", a.acl));
                    }
                    let in_incus = |f: fn(&ObservedAcl) -> &[AclRule]| {
                        found.map(|f2| f2.map(f).unwrap_or(&[]))
                    };
                    let ingress =
                        compare(&a.ingress, in_incus(|x| &x.ingress), "ingress", &mut mdrift);
                    let egress = compare(&a.egress, in_incus(|x| &x.egress), "egress", &mut mdrift);
                    let addresses = observed.map(|o| {
                        o.sets
                            .iter()
                            .find(|s| s.name == a.acl)
                            .map(|s| s.addresses.clone())
                            .unwrap_or_default()
                    });
                    if observed.is_some_and(|o| !o.sets.iter().any(|s| s.name == a.acl)) {
                        mdrift.push(format!("address set {} is not in Incus", a.acl));
                    }
                    NetModule {
                        internet: sync.internet.get(&a.module).copied().unwrap_or_default(),
                        module: a.module,
                        acl: a.acl,
                        ingress,
                        egress,
                        acl_present,
                        addresses,
                        drift: mdrift,
                    }
                })
                .collect()
        } else {
            Vec::new()
        };
        if let Some(o) = observed {
            let wanted: BTreeSet<&str> = modules.iter().map(|m| m.acl.as_str()).collect();
            for a in o
                .acls
                .iter()
                .filter(|a| a.application.as_deref() == Some(d.application.as_str()))
            {
                if !wanted.contains(a.name.as_str()) {
                    drift.push(format!(
                        "ACL {} belongs to the application but no module uses it",
                        a.name
                    ));
                }
            }
        }
        let services: Vec<NetService> = if enforced && d.service_range.is_some() {
            sync.services()
                .into_iter()
                .map(|m| {
                    let ports: Vec<NetPort> = sync
                        .ports_of(&m)
                        .into_iter()
                        .map(|(transport, port)| NetPort { transport, port })
                        .collect();
                    let address = addresses.get(&m).cloned();
                    let mut sdrift = Vec::new();
                    let lb = lbs.as_ref().and_then(|l| {
                        address
                            .as_ref()
                            .and_then(|a| l.iter().find(|lb| &lb.listen_address == a))
                    });
                    if lbs.is_some() {
                        match (&address, lb) {
                            (None, _) => sdrift.push("no service address is allocated yet".into()),
                            (Some(a), None) => {
                                sdrift.push(format!("load balancer {a} is not in Incus"))
                            }
                            (Some(_), Some(lb)) => {
                                let have: BTreeSet<(String, String)> =
                                    lb.ports.iter().cloned().collect();
                                let want: BTreeSet<(String, String)> = ports
                                    .iter()
                                    .map(|p| (p.transport.clone(), p.port.to_string()))
                                    .collect();
                                if have != want {
                                    sdrift
                                        .push("its ports in Incus differ from the design's".into());
                                }
                                if lb.backends.is_empty() {
                                    sdrift.push(
                                        "it has no backends: no replica has an address".into(),
                                    );
                                }
                            }
                        }
                    }
                    NetService {
                        published: published.iter().any(|x| x.module == m),
                        backends: lb.map(|lb| {
                            lb.backends
                                .iter()
                                .map(|b| NetBackend {
                                    name: b.name.clone(),
                                    address: b.target_address.clone(),
                                    instance: b
                                        .description
                                        .split(' ')
                                        .next()
                                        .unwrap_or_default()
                                        .to_owned(),
                                })
                                .collect()
                        }),
                        module: m,
                        address,
                        ports,
                        drift: sdrift,
                    }
                })
                .collect()
        } else {
            Vec::new()
        };
        if let Some(l) = &lbs {
            let held: BTreeSet<&String> = addresses.values().collect();
            for lb in l
                .iter()
                .filter(|lb| lb.application.as_deref() == Some(d.application.as_str()))
            {
                if !held.contains(&lb.listen_address) {
                    drift.push(format!(
                        "load balancer {} belongs to the application but holds no service address",
                        lb.listen_address
                    ));
                }
            }
        }
        items.push(NetApplication {
            application: d.application.clone(),
            project: d.project.clone(),
            revision: d.revision,
            enforced,
            network,
            uplink: d.uplink.clone(),
            service_range: d.service_range.clone(),
            network_observed,
            connections,
            published,
            modules,
            services,
            drift,
        });
    }
    Ok(Json(NetApplications {
        source: source(state.provider.mode()),
        incus_error,
        items,
    }))
}
