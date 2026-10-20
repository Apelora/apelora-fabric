// SPDX-License-Identifier: AGPL-3.0-only
//! `network.sync`: makes an application's network objects match its connections.
//!
//! Idempotent and level-triggered: ensures the application's OVN network and, per appliance,
//! an ACL and an address set; writes every ACL's rules from the connections (both sides);
//! fills each address set with its instance's current addresses. While an instance has no
//! address yet (still starting) the job retries; until then its set is empty, which denies.

use apelora_domain::is_valid_name;
use apelora_domain::network::{Connection, acl_name, acl_rules, network_name};
use apelora_domain::plan::replica_name;
use apelora_incus_adapter::{AdapterError, NetworkObject, Provider};
use apelora_persistence::Outcome;
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkSync {
    pub project: String,
    pub application: String,
    pub uplink: String,
    pub appliances: Vec<String>,
    pub connections: Vec<Connection>,
    /// Replicas per appliance (1 when absent): every replica's addresses join the set.
    #[serde(default)]
    pub replicas: std::collections::BTreeMap<String, u8>,
}

impl NetworkSync {
    pub fn from_payload(payload: &serde_json::Value) -> Result<Self, String> {
        let s: NetworkSync =
            serde_json::from_value(payload.clone()).map_err(|e| format!("invalid payload: {e}"))?;
        for n in [&s.project, &s.application, &s.uplink]
            .into_iter()
            .chain(&s.appliances)
        {
            if !is_valid_name(n) {
                return Err(format!("invalid name {n:?}"));
            }
        }
        for c in &s.connections {
            c.validate()?;
            if !s.appliances.contains(&c.from) || !s.appliances.contains(&c.to) {
                return Err(format!(
                    "connection {} → {} names an appliance outside the application",
                    c.from, c.to
                ));
            }
        }
        Ok(s)
    }
}

pub async fn sync(provider: &Provider, s: &NetworkSync) -> Outcome {
    match apply(provider, s).await {
        Ok(missing) if missing.is_empty() => Outcome::Succeeded,
        Ok(missing) => Outcome::Failed(format!("waiting for addresses of {}", missing.join(", "))),
        Err(e) => Outcome::Failed(e.to_string()),
    }
}

/// Returns the appliances whose instances have no address yet.
async fn apply(provider: &Provider, s: &NetworkSync) -> Result<Vec<String>, AdapterError> {
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
    remove_stale(provider, s).await?;
    Ok(missing)
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
