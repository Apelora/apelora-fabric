// SPDX-License-Identifier: AGPL-3.0-only
//! `network.sync`: makes an application's network objects match its connections.
//!
//! Idempotent and level-triggered: ensures the application's OVN network and, per appliance,
//! an ACL and an address set; writes every ACL's rules from the connections (both sides);
//! fills each address set with its instance's current addresses. While an instance has no
//! address yet (still starting) the job retries; until then its set is empty, which denies.

use apelora_domain::is_valid_name;
use apelora_domain::network::{Connection, acl_name, acl_rules, network_name};
use apelora_domain::plan::instance_name;
use apelora_incus_adapter::{AdapterError, Provider};
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
    let p = &s.project;
    provider
        .ensure_network(p, &network_name(&s.application), &s.uplink)
        .await?;
    let mut missing = Vec::new();
    for a in &s.appliances {
        let name = acl_name(&s.application, a);
        provider.ensure_acl(p, &name).await?;
        provider.ensure_address_set(p, &name).await?;
        let addresses = provider
            .instance_addresses(p, &instance_name(&s.application, a))
            .await?;
        if addresses.is_empty() {
            missing.push(a.clone());
        }
        provider.set_addresses(p, &name, &addresses).await?;
    }
    for a in &s.appliances {
        let (ingress, egress) = acl_rules(&s.application, a, &s.connections);
        provider
            .set_acl_rules(p, &acl_name(&s.application, a), &ingress, &egress)
            .await?;
    }
    Ok(missing)
}
