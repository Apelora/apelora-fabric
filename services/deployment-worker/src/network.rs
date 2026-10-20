// SPDX-License-Identifier: AGPL-3.0-only
//! `network.sync`: makes an application's network objects match its connections.
//!
//! Idempotent and level-triggered: ensures the application's OVN network and, per appliance,
//! an ACL and an address set; writes every ACL's rules from the connections (both sides);
//! fills each address set with its instance's current addresses. While an instance has no
//! address yet (still starting) the job retries; until then its set is empty, which denies.

use std::net::Ipv4Addr;

use apelora_domain::network::{
    Backend, LoadBalancer, SyncRequest, acl_name, acl_rules, network_name, service_addresses,
    service_ports,
};
use apelora_domain::plan::replica_name;
use apelora_incus_adapter::{AdapterError, NetworkObject, Provider};
use apelora_persistence::{Outcome, Pool};

/// The payload of a `network.sync` job.
pub type NetworkSync = SyncRequest;

pub fn from_payload(payload: &serde_json::Value) -> Result<NetworkSync, String> {
    let s: NetworkSync =
        serde_json::from_value(payload.clone()).map_err(|e| format!("invalid payload: {e}"))?;
    s.validate()?;
    Ok(s)
}

pub async fn sync(pool: &Pool, provider: &Provider, s: &NetworkSync) -> Outcome {
    match apply(pool, provider, s).await {
        Ok(missing) if missing.is_empty() => Outcome::Succeeded,
        Ok(missing) => Outcome::Failed(format!("waiting for {}", missing.join(", "))),
        Err(SyncError::Incus(e)) => Outcome::Failed(e.to_string()),
        Err(SyncError::Store(e)) => Outcome::Failed(format!("database: {e}")),
    }
}

enum SyncError {
    Incus(AdapterError),
    Store(apelora_persistence::StoreError),
}

impl From<AdapterError> for SyncError {
    fn from(e: AdapterError) -> Self {
        SyncError::Incus(e)
    }
}

impl From<apelora_persistence::StoreError> for SyncError {
    fn from(e: apelora_persistence::StoreError) -> Self {
        SyncError::Store(e)
    }
}

/// Returns what is not in place yet (instances without an address, services without one).
async fn apply(
    pool: &Pool,
    provider: &Provider,
    s: &NetworkSync,
) -> Result<Vec<String>, SyncError> {
    let (p, app) = (&s.project, s.application.as_str());
    let mut missing = Vec::new();
    if !s.appliances.is_empty() {
        provider
            .ensure_network(p, &network_name(app), &s.uplink, app)
            .await?;
    }
    for a in &s.appliances {
        let name = acl_name(app, a);
        provider.ensure_acl(p, &name, app).await?;
        provider.ensure_address_set(p, &name, app).await?;
        let mut addresses = Vec::new();
        for replica in 1..=s.replicas.get(a).copied().unwrap_or(1).max(1) {
            let instance = replica_name(app, a, replica);
            let found = provider.instance_addresses(p, &instance).await?;
            if found.is_empty() {
                missing.push(instance);
            }
            addresses.extend(found);
        }
        addresses.sort();
        provider.set_addresses(p, &name, &addresses, app).await?;
    }
    for a in &s.appliances {
        let (ingress, egress) = acl_rules(app, a, &s.connections);
        provider
            .set_acl_rules(p, &acl_name(app, a), &ingress, &egress, app)
            .await?;
    }
    missing.extend(services(pool, provider, s).await?);
    remove_stale(provider, s).await?;
    Ok(missing)
}

/// Gives every connected stateless appliance a service address: an OVN load balancer over its
/// replicas' IPv4 addresses on the ports it is reached on, with health checks. Removes the
/// application's load balancers (and frees their addresses) for appliances that no longer
/// get one. Returns the services that could not be set up yet.
async fn services(
    pool: &Pool,
    provider: &Provider,
    s: &NetworkSync,
) -> Result<Vec<String>, SyncError> {
    let (p, app) = (&s.project, s.application.as_str());
    let network = network_name(app);
    let application_id = apelora_persistence::ensure_application(pool, app).await?.id;
    let wanted = match &s.service_range {
        Some(_) => s.services(),
        None => Vec::new(),
    };
    let candidates: Vec<String> = match &s.service_range {
        Some(r) => service_addresses(r)
            .map_err(|e| {
                SyncError::Incus(AdapterError::Api {
                    status: 400,
                    message: e,
                })
            })?
            .iter()
            .map(ToString::to_string)
            .collect(),
        None => Vec::new(),
    };
    let mut waiting = Vec::new();
    let mut kept: Vec<String> = Vec::new();
    for a in &wanted {
        let Some(address) =
            apelora_persistence::service_address(pool, application_id, p, a, &candidates).await?
        else {
            waiting.push(format!(
                "a free service address for {a} (range {} is exhausted)",
                s.service_range.as_deref().unwrap_or("")
            ));
            continue;
        };
        let Ok(listen_address) = address.parse::<Ipv4Addr>() else {
            continue;
        };
        kept.push(listen_address.to_string());
        let mut backends = Vec::new();
        for replica in 1..=s.replicas.get(a).copied().unwrap_or(1).max(1) {
            let instance = replica_name(app, a, replica);
            let ipv4 = provider
                .instance_addresses(p, &instance)
                .await?
                .into_iter()
                .find_map(|a| {
                    a.strip_suffix("/32")
                        .and_then(|a| a.parse::<Ipv4Addr>().ok())
                });
            let Some(address) = ipv4 else { continue };
            let uuid = provider
                .instance_uuid(p, &instance)
                .await?
                .unwrap_or_default();
            backends.push(Backend {
                name: format!("r{replica}"),
                address,
                description: format!("{instance} {uuid}").trim().to_owned(),
            });
        }
        if backends.is_empty() {
            waiting.push(format!("an address of a {a} replica for its service"));
            continue;
        }
        let lb = LoadBalancer {
            listen_address,
            appliance: a.clone(),
            backends,
            ports: service_ports(a, &s.connections),
        };
        provider.ensure_load_balancer(p, &network, &lb, app).await?;
    }
    for (address, _) in provider.owned_load_balancers(p, &network, app).await? {
        if !kept.contains(&address) {
            provider
                .delete_load_balancer(p, &network, &address, app)
                .await?;
        }
    }
    apelora_persistence::release_service_addresses(pool, application_id, p, &wanted).await?;
    Ok(waiting)
}

/// Deletes the application's ACLs and address sets for appliances no longer in the design
/// (ACLs first: they reference the sets), and its network once no appliance is left. Only
/// objects labelled with this application are considered.
async fn remove_stale(provider: &Provider, s: &NetworkSync) -> Result<(), AdapterError> {
    let (p, app) = (&s.project, s.application.as_str());
    let current: Vec<String> = s.appliances.iter().map(|a| acl_name(app, a)).collect();
    for kind in [NetworkObject::Acl, NetworkObject::AddressSet] {
        let stale: Vec<String> = provider
            .owned_network_objects(kind, p, app)
            .await?
            .into_iter()
            .filter(|n| !current.contains(n))
            .collect();
        for name in stale {
            provider.delete_network_object(kind, p, &name, app).await?;
        }
    }
    if s.appliances.is_empty() {
        provider
            .delete_network_object(NetworkObject::Network, p, &network_name(app), app)
            .await?;
    }
    Ok(())
}
