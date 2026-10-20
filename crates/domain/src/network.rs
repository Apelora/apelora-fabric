// SPDX-License-Identifier: AGPL-3.0-only
//! Enforcing a design's connections on an OVN network (Phase 07).
//!
//! Every application gets one OVN network; every appliance an ACL and an address set holding
//! its instances' addresses. A connection lets the consumer reach the provider on one
//! protocol and port: the consumer's ACL allows it outbound and the provider's ACL allows it
//! inbound (Incus combines egress and ingress allows with OR, so both sides are always
//! written). Rules use address sets, not ACL selectors: selector rules only matched traffic
//! between instances on the same member (docs/development/TEST-CLUSTER.md).

use serde::{Deserialize, Serialize};

use crate::is_valid_name;
use crate::plan::instance_name;

/// One connection of the design: `from` may reach `to` (appliance ids).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    pub from: String,
    pub to: String,
    /// `tcp` or `udp`.
    pub transport: String,
    /// `None`: any port of the transport (the protocol has no well-known port).
    pub port: Option<u16>,
}

impl Connection {
    pub fn validate(&self) -> Result<(), String> {
        for (what, v) in [("from", &self.from), ("to", &self.to)] {
            if !is_valid_name(v) {
                return Err(format!("connection {what}: invalid appliance id {v:?}"));
            }
        }
        if self.from == self.to {
            return Err(format!("connection {} → itself", self.from));
        }
        if !matches!(self.transport.as_str(), "tcp" | "udp") {
            return Err(format!(
                "connection {} → {}: transport must be tcp or udp",
                self.from, self.to
            ));
        }
        if self.port == Some(0) {
            return Err(format!("connection {} → {}: port 0", self.from, self.to));
        }
        Ok(())
    }
}

/// Where an instance's NIC is attached: the application's network, with its own ACL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct NetworkAttachment {
    pub network: String,
    pub acl: String,
    /// Uplink network the application network is created on.
    pub uplink: String,
}

/// One ACL rule in Incus's terms.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AclRule {
    pub action: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub source: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub destination: String,
    pub protocol: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub destination_port: String,
    pub state: String,
    pub description: String,
}

/// The application's network name: the application name itself (OVN networks are not
/// limited to interface-name length).
pub fn network_name(application: &str) -> String {
    application.to_owned()
}

/// The ACL and the address set of an appliance share the instance's name.
pub fn acl_name(application: &str, appliance: &str) -> String {
    instance_name(application, appliance)
}

pub fn attachment(application: &str, appliance: &str, uplink: &str) -> NetworkAttachment {
    NetworkAttachment {
        network: network_name(application),
        acl: acl_name(application, appliance),
        uplink: uplink.to_owned(),
    }
}

/// The ingress and egress rules of `appliance`'s ACL, sorted so unchanged designs produce
/// identical rules.
pub fn acl_rules(
    application: &str,
    appliance: &str,
    connections: &[Connection],
) -> (Vec<AclRule>, Vec<AclRule>) {
    let rule = |source: String, destination: String, c: &Connection, description: String| AclRule {
        action: "allow".into(),
        source,
        destination,
        protocol: c.transport.clone(),
        destination_port: c.port.map(|p| p.to_string()).unwrap_or_default(),
        state: "enabled".into(),
        description,
    };
    let set = |a: &str| format!("${}", acl_name(application, a));
    let mut ingress: Vec<AclRule> = connections
        .iter()
        .filter(|c| c.to == appliance)
        .map(|c| rule(set(&c.from), String::new(), c, format!("from {}", c.from)))
        .collect();
    let mut egress: Vec<AclRule> = connections
        .iter()
        .filter(|c| c.from == appliance)
        .map(|c| rule(String::new(), set(&c.to), c, format!("to {}", c.to)))
        .collect();
    let key = |r: &AclRule| {
        (
            r.source.clone(),
            r.destination.clone(),
            r.protocol.clone(),
            r.destination_port.clone(),
        )
    };
    ingress.sort_by_key(key);
    ingress.dedup();
    egress.sort_by_key(key);
    egress.dedup();
    (ingress, egress)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(from: &str, to: &str, port: Option<u16>) -> Connection {
        Connection {
            from: from.into(),
            to: to.into(),
            transport: "tcp".into(),
            port,
        }
    }

    #[test]
    fn both_sides_get_matching_rules_by_address_set() {
        let conns = [
            c("web-01", "db-01", Some(5432)),
            c("lb-01", "web-01", Some(80)),
            c("web-01", "db-01", Some(5432)),
        ];
        let (ingress, egress) = acl_rules("shop", "web-01", &conns);
        assert_eq!(ingress.len(), 1);
        assert_eq!(
            (
                ingress[0].source.as_str(),
                ingress[0].destination_port.as_str()
            ),
            ("$shop-lb-01", "80")
        );
        assert_eq!(egress.len(), 1, "duplicates collapse");
        assert_eq!(
            (egress[0].destination.as_str(), egress[0].protocol.as_str()),
            ("$shop-db-01", "tcp")
        );
        let (db_in, db_out) = acl_rules("shop", "db-01", &conns);
        assert_eq!(db_in[0].source, "$shop-web-01");
        assert!(
            db_out.is_empty(),
            "providers get no outbound rules of their own"
        );
        let (lonely_in, lonely_out) = acl_rules("shop", "cache-01", &conns);
        assert!(
            lonely_in.is_empty() && lonely_out.is_empty(),
            "unconnected appliances are isolated"
        );
    }

    #[test]
    fn any_port_rules_omit_the_port_and_connections_are_validated() {
        let (ingress, _) = acl_rules("shop", "raw-01", &[c("app-01", "raw-01", None)]);
        assert_eq!(ingress[0].destination_port, "");
        assert!(c("a", "a", Some(1)).validate().is_err());
        assert!(c("a", "b", Some(0)).validate().is_err());
        assert!(
            Connection {
                transport: "icmp".into(),
                ..c("a", "b", None)
            }
            .validate()
            .is_err()
        );
        assert!(c("../x", "b", Some(1)).validate().is_err());
        assert_eq!(c("a", "b", Some(443)).validate(), Ok(()));
        assert_eq!(
            attachment("shop", "web-01", "incusbr0"),
            NetworkAttachment {
                network: "shop".into(),
                acl: "shop-web-01".into(),
                uplink: "incusbr0".into()
            }
        );
    }
}
