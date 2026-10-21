// SPDX-License-Identifier: AGPL-3.0-only
//! `network.sync`: makes an application's network objects match its connections.
//!
//! Idempotent and level-triggered: ensures the application's OVN network and, per module,
//! an ACL and an address set; writes every ACL's rules from the connections (both sides);
//! fills each address set with its instance's current addresses. While an instance has no
//! address yet (still starting) the job retries; until then its set is empty, which denies.

use std::net::Ipv4Addr;

use apelora_domain::Instance;
use apelora_domain::network::{
    Backend, LoadBalancer, SyncRequest, acl_name, network_name, service_addresses,
};
use apelora_domain::plan::{replica_name, slot_instances};
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
        // Normal while instances start: reported as a failed attempt so the retry limit still
        // ends a sync that can never complete (an instance that never gets an address).
        Ok(missing) => {
            let waiting = missing.join(", ");
            tracing::info!(application = %s.application, %waiting, "network: not everything has an address yet; the sync is retried");
            Outcome::Failed(format!("waiting for {waiting}"))
        }
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
    if !s.modules.is_empty() {
        provider
            .ensure_network(p, &network_name(app), &s.uplink, app)
            .await?;
    }
    let inv = Inventory::read(provider).await?;
    for a in &s.modules {
        let name = acl_name(app, a);
        provider.ensure_acl(p, &name, app).await?;
        provider.ensure_address_set(p, &name, app).await?;
        let mut addresses = Vec::new();
        for replica in 1..=s.replicas.get(a).copied().unwrap_or(1).max(1) {
            let slot = inv.slot(s, a, replica);
            if slot.is_empty() {
                missing.push(replica_name(app, a, replica));
            }
            let mut found_any = slot.iter().all(|(_, reachable)| !reachable);
            for (instance, reachable) in &slot {
                if !reachable {
                    continue;
                }
                let found = provider.instance_addresses(p, instance).await?;
                found_any |= !found.is_empty();
                addresses.extend(found);
            }
            if !slot.is_empty() && !found_any {
                missing.push(replica_name(app, a, replica));
            }
        }
        addresses.sort();
        provider.set_addresses(p, &name, &addresses, app).await?;
    }
    // The same rules the Console's Networks page shows as intended (`intended_acls`).
    for a in s.intended_acls() {
        provider
            .set_acl_rules(p, &a.acl, &a.ingress, &a.egress, app)
            .await?;
    }
    missing.extend(services(pool, provider, s, &inv).await?);
    remove_stale(provider, s).await?;
    Ok(missing)
}

/// Gives every connected stateless module a service address: an OVN load balancer over its
/// replicas' IPv4 addresses on the ports it is reached on, with health checks. Removes the
/// application's load balancers (and frees their addresses) for modules that no longer
/// get one. Returns the services that could not be set up yet.
async fn services(
    pool: &Pool,
    provider: &Provider,
    s: &NetworkSync,
    inv: &Inventory,
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
            let base = replica_name(app, a, replica);
            for (instance, reachable) in inv.slot(s, a, replica) {
                if !reachable {
                    continue;
                }
                let ipv4 = provider
                    .instance_addresses(p, &instance)
                    .await?
                    .into_iter()
                    .find_map(|a| {
                        a.strip_suffix("/32")
                            .and_then(|a| a.parse::<Ipv4Addr>().ok())
                    });
                let Some(address) = ipv4 else { continue };
                // Only describes the backend: a missing UUID is logged, not fatal.
                let uuid = provider
                    .instance_uuid(p, &instance)
                    .await?
                    .unwrap_or_else(|| {
                        tracing::warn!(instance = %format!("{p}/{instance}"), "network: Incus reports no UUID for the instance; its service backend is described without one");
                        String::new()
                    });
                backends.push(Backend {
                    // `r2`, or `r2-s1` for a substitute of replica 2.
                    name: format!("r{replica}{}", instance.strip_prefix(&base).unwrap_or("")),
                    address,
                    description: format!("{instance} {uuid}").trim().to_owned(),
                });
            }
        }
        if backends.is_empty() {
            waiting.push(format!("an address of a {a} replica for its service"));
            continue;
        }
        let lb = LoadBalancer {
            listen_address,
            module: a.clone(),
            backends,
            ports: s.ports_of(a),
        };
        let changed = provider.ensure_load_balancer(p, &network, &lb, app).await?;
        // Incus 7.0.1 can drop the health check in OVN on an update while still reporting it
        // configured; its state then says `unknown`. Re-apply it after every change and
        // whenever a backend reports no check.
        let address = lb.listen_address.to_string();
        let unchecked = provider
            .load_balancer_health(p, &network, &address)
            .await?
            .iter()
            .any(|(_, status)| status == "unknown");
        if changed || unchecked {
            provider
                .reapply_health_check(p, &network, &address, app)
                .await?;
        }
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

/// Deletes the application's ACLs and address sets for modules no longer in the design
/// (ACLs first: they reference the sets), and its network once no module is left. Only
/// objects labelled with this application are considered.
async fn remove_stale(provider: &Provider, s: &NetworkSync) -> Result<(), AdapterError> {
    let (p, app) = (&s.project, s.application.as_str());
    let current: Vec<String> = s.modules.iter().map(|a| acl_name(app, a)).collect();
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
    if s.modules.is_empty() {
        provider
            .delete_network_object(NetworkObject::Network, p, &network_name(app), app)
            .await?;
    }
    Ok(())
}

/// The instances Incus reports, and the members that cannot be asked about theirs.
struct Inventory {
    observed: Vec<Instance>,
    unavailable: Vec<String>,
}

impl Inventory {
    async fn read(provider: &Provider) -> Result<Self, AdapterError> {
        let observed = provider.instances().await?;
        let unavailable = provider
            .cluster_members()
            .await
            .map(|m| {
                m.into_iter()
                    .filter(|m| m.status != "Online")
                    .map(|m| m.name)
                    .collect()
            })
            .unwrap_or_default();
        Ok(Inventory {
            observed,
            unavailable,
        })
    }

    /// The instances filling replica `replica` of `module` (the replica and its
    /// substitutes), each with whether its member can be reached.
    fn slot(&self, s: &NetworkSync, module: &str, replica: u8) -> Vec<(String, bool)> {
        slot_instances(&s.application, &s.project, module, replica, &self.observed)
            .into_iter()
            .map(|i| (i.name.clone(), !self.unavailable.contains(&i.location)))
            .collect()
    }
}
