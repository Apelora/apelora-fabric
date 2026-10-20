// SPDX-License-Identifier: AGPL-3.0-only
//! Enforcing a design's connections on an OVN network (Phase 07).
//!
//! Every application gets one OVN network; every appliance an ACL and an address set holding
//! its instances' addresses. A connection lets the consumer reach the provider on one
//! protocol and port: the consumer's ACL allows it outbound and the provider's ACL allows it
//! inbound (Incus combines egress and ingress allows with OR, so both sides are always
//! written). Rules use address sets, not ACL selectors: selector rules only matched traffic
//! between instances on the same member (docs/development/TEST-CLUSTER.md).
//!
//! A stateless appliance that others connect to also gets a *service address*: an OVN load
//! balancer on the application's network, listening on an address from a routed range and
//! spreading each connected port over the appliance's replicas, with health checks. The load
//! balancer translates the address before ACLs are evaluated, so the address-set rules above
//! already cover traffic to it.

use std::collections::BTreeMap;
use std::net::Ipv4Addr;

use serde::{Deserialize, Serialize};

use crate::is_valid_name;
use crate::plan::{Availability, DesiredAppliance, instance_name};

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

/// What an appliance may reach outside the application (the internet, through the uplink).
/// Inside the application only connections count, whatever this says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum Internet {
    /// Nothing outside.
    #[default]
    None,
    /// DNS, HTTP and HTTPS (package mirrors, APIs).
    Web,
    /// Any port outside.
    Any,
}

/// Egress rules for `level`. They use Incus's `@external` subject (traffic leaving through the
/// uplink), never "any destination": an egress allow alone would let traffic into other
/// appliances of the application (REAL INCUS TESTED, TEST-CLUSTER.md).
pub fn internet_rules(level: Internet) -> Vec<AclRule> {
    let rule = |protocol: &str, ports: &str, description: &str| AclRule {
        action: "allow".into(),
        source: String::new(),
        destination: "@external".into(),
        protocol: protocol.into(),
        destination_port: ports.into(),
        state: "enabled".into(),
        description: description.into(),
    };
    match level {
        Internet::None => vec![],
        Internet::Web => vec![
            rule("tcp", "80,443", "internet: web"),
            rule("udp", "53", "internet: DNS"),
            rule("tcp", "53", "internet: DNS"),
        ],
        Internet::Any => vec![
            rule("tcp", "", "internet: any"),
            rule("udp", "", "internet: any"),
            rule("icmp4", "", "internet: any"),
        ],
    }
}

/// A port of an appliance reachable from outside the application (in the Studio: connected
/// from a gateway). Only ports with a number can be published.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct Published {
    pub appliance: String,
    /// `tcp` or `udp`.
    pub transport: String,
    pub port: u16,
}

impl Published {
    pub fn validate(&self) -> Result<(), String> {
        if !is_valid_name(&self.appliance) {
            return Err(format!(
                "published: invalid appliance id {:?}",
                self.appliance
            ));
        }
        if !matches!(self.transport.as_str(), "tcp" | "udp") || self.port == 0 {
            return Err(format!(
                "published {}: needs tcp or udp and a port",
                self.appliance
            ));
        }
        Ok(())
    }
}

/// Ingress rules that let anyone reach `appliance`'s published ports. Traffic still has to be
/// routed to the appliance's service address by the network outside (TEST-CLUSTER.md).
pub fn published_rules(appliance: &str, published: &[Published]) -> Vec<AclRule> {
    let mut rules: Vec<AclRule> = published
        .iter()
        .filter(|p| p.appliance == appliance)
        .map(|p| AclRule {
            action: "allow".into(),
            source: String::new(),
            destination: String::new(),
            protocol: p.transport.clone(),
            destination_port: p.port.to_string(),
            state: "enabled".into(),
            description: "published".into(),
        })
        .collect();
    rules.sort_by(|a, b| {
        (&a.protocol, &a.destination_port).cmp(&(&b.protocol, &b.destination_port))
    });
    rules.dedup();
    rules
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

/// Everything `network.sync` needs for one application in one project (the job's payload),
/// built by the control plane on deployment and by the replica supervisor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncRequest {
    pub project: String,
    pub application: String,
    pub uplink: String,
    pub appliances: Vec<String>,
    pub connections: Vec<Connection>,
    /// Replicas per appliance (1 when absent): every replica's addresses join the set.
    #[serde(default)]
    pub replicas: BTreeMap<String, u8>,
    /// Stateless appliances: those connected to get a service address.
    #[serde(default)]
    pub stateless: Vec<String>,
    /// IPv4 range service addresses come from (routed by the uplink); none: no load balancers.
    #[serde(default)]
    pub service_range: Option<String>,
    /// Ports reachable from outside the application.
    #[serde(default)]
    pub published: Vec<Published>,
    /// What each appliance may reach outside the application (absent: nothing).
    #[serde(default)]
    pub internet: BTreeMap<String, Internet>,
}

impl SyncRequest {
    pub fn for_deployment(
        application: &str,
        project: &str,
        uplink: &str,
        desired: &[DesiredAppliance],
        connections: &[Connection],
        service_range: Option<&str>,
    ) -> Self {
        SyncRequest {
            project: project.to_owned(),
            application: application.to_owned(),
            uplink: uplink.to_owned(),
            appliances: desired.iter().map(|d| d.appliance.clone()).collect(),
            connections: connections.to_vec(),
            replicas: desired
                .iter()
                .map(|d| (d.appliance.clone(), d.replicas))
                .collect(),
            stateless: desired
                .iter()
                .filter(|d| d.availability == Availability::Stateless)
                .map(|d| d.appliance.clone())
                .collect(),
            service_range: service_range.map(str::to_owned),
            published: Vec::new(),
            internet: desired
                .iter()
                .filter(|d| d.internet != Internet::None)
                .map(|d| (d.appliance.clone(), d.internet))
                .collect(),
        }
    }

    pub fn with_published(mut self, published: &[Published]) -> Self {
        self.published = published.to_vec();
        self
    }

    pub fn validate(&self) -> Result<(), String> {
        for n in [&self.project, &self.application, &self.uplink]
            .into_iter()
            .chain(&self.appliances)
        {
            if !is_valid_name(n) {
                return Err(format!("invalid name {n:?}"));
            }
        }
        for c in &self.connections {
            c.validate()?;
            if !self.appliances.contains(&c.from) || !self.appliances.contains(&c.to) {
                return Err(format!(
                    "connection {} → {} names an appliance outside the application",
                    c.from, c.to
                ));
            }
        }
        if let Some(r) = &self.service_range {
            service_addresses(r)?;
        }
        for p in &self.published {
            p.validate()?;
            if !self.appliances.contains(&p.appliance) {
                return Err(format!(
                    "published {} is not in the application",
                    p.appliance
                ));
            }
        }
        Ok(())
    }

    /// The ports a service address of `appliance` listens on: those other appliances connect
    /// to and those published.
    pub fn ports_of(&self, appliance: &str) -> Vec<(String, u16)> {
        let mut ports = service_ports(appliance, &self.connections);
        ports.extend(
            self.published
                .iter()
                .filter(|p| p.appliance == appliance)
                .map(|p| (p.transport.clone(), p.port)),
        );
        ports.sort();
        ports.dedup();
        ports
    }

    /// Appliances that get a service address: stateless ones reached on at least one port,
    /// and every published one (outside clients need one stable address, even for a single
    /// instance).
    pub fn services(&self) -> Vec<String> {
        self.appliances
            .iter()
            .filter(|a| {
                self.published.iter().any(|p| &p.appliance == *a)
                    || self.stateless.contains(a) && !service_ports(a, &self.connections).is_empty()
            })
            .cloned()
            .collect()
    }
}

/// The host addresses of an IPv4 range given as `a.b.c.d/len` (16 ≤ len ≤ 30), lowest first,
/// without the network and broadcast addresses.
pub fn service_addresses(range: &str) -> Result<Vec<Ipv4Addr>, String> {
    let bad = || {
        format!("service range {range:?}: expected an IPv4 range like 198.18.100.0/28 (/16 to /30)")
    };
    let (addr, len) = range.split_once('/').ok_or_else(bad)?;
    let addr: Ipv4Addr = addr.parse().map_err(|_| bad())?;
    let len: u32 = len.parse().map_err(|_| bad())?;
    if !(16..=30).contains(&len) {
        return Err(bad());
    }
    let mask = u32::MAX << (32 - len);
    let network = u32::from(addr) & mask;
    if network != u32::from(addr) {
        return Err(format!(
            "service range {range:?}: host bits are set (the range starts at {})",
            Ipv4Addr::from(network)
        ));
    }
    Ok((network + 1..network + (1 << (32 - len)) - 1)
        .map(Ipv4Addr::from)
        .collect())
}

/// The ports `appliance` is reached on, as `(transport, port)`, sorted. Connections without a
/// well-known port allow any port and cannot be balanced, so they are left out.
pub fn service_ports(appliance: &str, connections: &[Connection]) -> Vec<(String, u16)> {
    let mut ports: Vec<(String, u16)> = connections
        .iter()
        .filter(|c| c.to == appliance)
        .filter_map(|c| c.port.map(|p| (c.transport.clone(), p)))
        .collect();
    ports.sort();
    ports.dedup();
    ports
}

/// One replica behind a service address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backend {
    /// `r<replica>`.
    pub name: String,
    pub address: Ipv4Addr,
    /// `<instance> <instance uuid>`: a recreated replica changes it even at the same address,
    /// which makes Incus rebind the health check (docs/development/TEST-CLUSTER.md).
    pub description: String,
}

/// An appliance's service address: an OVN load balancer on the application's network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadBalancer {
    pub listen_address: Ipv4Addr,
    pub appliance: String,
    pub backends: Vec<Backend>,
    pub ports: Vec<(String, u16)>,
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

    #[test]
    fn service_ranges_skip_network_and_broadcast_and_reject_odd_input() {
        let a = service_addresses("198.18.100.0/29").expect("range");
        assert_eq!(
            a.first().map(ToString::to_string).as_deref(),
            Some("198.18.100.1")
        );
        assert_eq!(
            a.last().map(ToString::to_string).as_deref(),
            Some("198.18.100.6")
        );
        assert_eq!(a.len(), 6);
        for bad in [
            "198.18.100.0",
            "198.18.100.1/28",
            "198.18.100.0/31",
            "10.0.0.0/8",
            "::1/64",
            "x/28",
        ] {
            assert!(service_addresses(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn only_connected_stateless_appliances_get_a_service_with_their_ports() {
        let mut conns = vec![
            c("lb-01", "web-01", Some(80)),
            c("lb-01", "web-01", Some(443)),
            c("web-01", "db-01", Some(5432)),
            c("app-01", "raw-01", None),
        ];
        conns.push(c("lb-01", "web-01", Some(80)));
        assert_eq!(
            service_ports("web-01", &conns),
            vec![("tcp".to_owned(), 80), ("tcp".to_owned(), 443)]
        );
        let s = SyncRequest {
            project: "apelora".into(),
            application: "shop".into(),
            uplink: "incusbr0".into(),
            appliances: ["lb-01", "web-01", "db-01", "app-01", "raw-01"]
                .map(String::from)
                .to_vec(),
            connections: conns,
            replicas: BTreeMap::new(),
            stateless: ["web-01", "lb-01", "raw-01"].map(String::from).to_vec(),
            service_range: Some("198.18.100.0/28".into()),
            published: vec![],
            internet: BTreeMap::new(),
        };
        assert_eq!(s.validate(), Ok(()));
        // lb-01: stateless but nobody connects to it; db-01: connected but stateful; raw-01:
        // reached on any port, which cannot be balanced.
        assert_eq!(s.services(), vec!["web-01".to_owned()]);
        assert!(
            SyncRequest {
                service_range: Some("198.18.100.1/28".into()),
                ..s
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn published_ports_open_to_anyone_and_get_a_service_address() {
        let published = vec![Published {
            appliance: "db-01".into(),
            transport: "tcp".into(),
            port: 5432,
        }];
        let rules = published_rules("db-01", &published);
        assert_eq!(rules.len(), 1);
        assert_eq!(
            (rules[0].source.as_str(), rules[0].destination_port.as_str()),
            ("", "5432")
        );
        assert!(published_rules("web-01", &published).is_empty());
        let s = SyncRequest {
            project: "apelora".into(),
            application: "shop".into(),
            uplink: "incusbr0".into(),
            appliances: ["web-01", "db-01"].map(String::from).to_vec(),
            connections: vec![c("web-01", "db-01", Some(5432))],
            replicas: BTreeMap::new(),
            stateless: vec![],
            service_range: Some("198.18.100.0/28".into()),
            published,
            internet: BTreeMap::new(),
        };
        assert_eq!(s.validate(), Ok(()));
        assert_eq!(
            s.services(),
            vec!["db-01".to_owned()],
            "published, although not stateless"
        );
        assert_eq!(s.ports_of("db-01"), vec![("tcp".to_owned(), 5432)]);
        let bad = SyncRequest {
            published: vec![Published {
                appliance: "nope-01".into(),
                transport: "tcp".into(),
                port: 1,
            }],
            ..s.clone()
        };
        assert!(bad.validate().is_err());
        assert!(
            Published {
                appliance: "db-01".into(),
                transport: "icmp".into(),
                port: 1
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn internet_access_only_ever_targets_external_traffic() {
        assert!(internet_rules(Internet::None).is_empty());
        for level in [Internet::Web, Internet::Any] {
            let rules = internet_rules(level);
            assert!(!rules.is_empty());
            assert!(
                rules
                    .iter()
                    .all(|r| r.destination == "@external" && r.source.is_empty()),
                "{rules:?}"
            );
        }
        assert!(
            internet_rules(Internet::Web)
                .iter()
                .any(|r| r.destination_port == "80,443")
        );
        assert_eq!(
            serde_json::to_value(Internet::Web).expect("json"),
            serde_json::json!("web")
        );
    }
}
