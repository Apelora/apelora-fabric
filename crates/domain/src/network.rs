// SPDX-License-Identifier: AGPL-3.0-only
//! Enforcing a design's connections on an OVN network (Phase 07).
//!
//! Every application gets one OVN network; every module an ACL and an address set holding
//! its instances' addresses. A connection lets the consumer reach the provider on one
//! protocol and port: the consumer's ACL allows it outbound and the provider's ACL allows it
//! inbound (Incus combines egress and ingress allows with OR, so both sides are always
//! written). Rules use address sets, not ACL selectors: selector rules only matched traffic
//! between instances on the same member (docs/development/TEST-CLUSTER.md).
//!
//! A stateless module that others connect to also gets a *service address*: an OVN load
//! balancer on the application's network, listening on an address from a routed range and
//! spreading each connected port over the module's replicas, with health checks. The load
//! balancer translates the address before ACLs are evaluated, so the address-set rules above
//! already cover traffic to it.

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use serde::{Deserialize, Serialize};

use crate::is_valid_name;
use crate::plan::{Availability, DesiredModule, instance_name};

/// One connection of the design: `from` may reach `to` (module ids).
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
                return Err(format!("connection {what}: invalid module id {v:?}"));
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

/// What a module may reach outside the application (the internet, through the uplink).
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
/// modules of the application (REAL INCUS TESTED, TEST-CLUSTER.md).
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

/// The environment variable that tells a consumer where `provider` is (ADR 0004):
/// `APELORA_SERVICE_<PROVIDER>`, upper case, `-` as `_`.
pub fn service_env_name(provider: &str) -> String {
    format!(
        "APELORA_SERVICE_{}",
        provider.to_ascii_uppercase().replace('-', "_")
    )
}

/// The discovery environment of `consumer` (ADR 0004): one variable per module it connects
/// to, holding that module's service address when it has one (`addresses`), otherwise its
/// instance name (resolved by the network's DNS).
pub fn discovery_env(
    application: &str,
    consumer: &str,
    connections: &[Connection],
    addresses: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    for c in connections.iter().filter(|c| c.from == consumer) {
        let target = addresses
            .get(&c.to)
            .cloned()
            .unwrap_or_else(|| instance_name(application, &c.to));
        env.insert(service_env_name(&c.to), target);
        // A service address is not an instance name, so OVN's DNS does not answer it.
        // The name is published on the network (`user.apelora.dns`) for a resolver.
        if addresses.contains_key(&c.to) {
            env.insert(
                crate::dns::name_env(&c.to),
                crate::dns::service_name(&c.to, application),
            );
        }
    }
    env
}

/// Adds the discovery environment to an instance's configuration (`environment.*`).
pub fn with_discovery(
    mut spec: crate::InstanceSpec,
    env: &BTreeMap<String, String>,
) -> crate::InstanceSpec {
    for (k, v) in env {
        spec.config.insert(format!("environment.{k}"), v.clone());
    }
    spec
}

/// A port of a module reachable from outside the application (in the Studio: connected
/// from a gateway). Only ports with a number can be published.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct Published {
    #[serde(alias = "appliance")]
    pub module: String,
    /// `tcp` or `udp`.
    pub transport: String,
    pub port: u16,
}

impl Published {
    pub fn validate(&self) -> Result<(), String> {
        if !is_valid_name(&self.module) {
            return Err(format!("published: invalid module id {:?}", self.module));
        }
        if !matches!(self.transport.as_str(), "tcp" | "udp") || self.port == 0 {
            return Err(format!(
                "published {}: needs tcp or udp and a port",
                self.module
            ));
        }
        Ok(())
    }
}

/// Ingress rules that let anyone reach `module`'s published ports. Traffic still has to be
/// routed to the module's service address by the network outside (TEST-CLUSTER.md).
pub fn published_rules(module: &str, published: &[Published]) -> Vec<AclRule> {
    let mut rules: Vec<AclRule> = published
        .iter()
        .filter(|p| p.module == module)
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

/// The ACL and the address set of a module share the instance's name.
pub fn acl_name(application: &str, module: &str) -> String {
    instance_name(application, module)
}

pub fn attachment(application: &str, module: &str, uplink: &str) -> NetworkAttachment {
    NetworkAttachment {
        network: network_name(application),
        acl: acl_name(application, module),
        uplink: uplink.to_owned(),
    }
}

/// The ingress and egress rules of `module`'s ACL, sorted so unchanged designs produce
/// identical rules.
pub fn acl_rules(
    application: &str,
    module: &str,
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
        .filter(|c| c.to == module)
        .map(|c| rule(set(&c.from), String::new(), c, format!("from {}", c.from)))
        .collect();
    let mut egress: Vec<AclRule> = connections
        .iter()
        .filter(|c| c.from == module)
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
    #[serde(alias = "appliances")]
    pub modules: Vec<String>,
    pub connections: Vec<Connection>,
    /// Replicas per module (1 when absent): every replica's addresses join the set.
    #[serde(default)]
    pub replicas: BTreeMap<String, u8>,
    /// Stateless modules: those connected to get a service address.
    #[serde(default)]
    pub stateless: Vec<String>,
    /// IPv4 range service addresses come from (routed by the uplink); none: no load balancers.
    #[serde(default)]
    pub service_range: Option<String>,
    /// Ports reachable from outside the application.
    #[serde(default)]
    pub published: Vec<Published>,
    /// What each module may reach outside the application (absent: nothing).
    #[serde(default)]
    pub internet: BTreeMap<String, Internet>,
    /// Modules that answer service names inside the application (ADR 0004 step 4). Empty
    /// unless the design includes a resolver: nothing is added to every application.
    #[serde(default)]
    pub resolvers: Vec<String>,
}

impl SyncRequest {
    pub fn for_deployment(
        application: &str,
        project: &str,
        uplink: &str,
        desired: &[DesiredModule],
        connections: &[Connection],
        service_range: Option<&str>,
    ) -> Self {
        SyncRequest {
            project: project.to_owned(),
            application: application.to_owned(),
            uplink: uplink.to_owned(),
            modules: desired.iter().map(|d| d.module.clone()).collect(),
            connections: connections.to_vec(),
            replicas: desired
                .iter()
                .map(|d| (d.module.clone(), d.replicas))
                .collect(),
            stateless: desired
                .iter()
                .filter(|d| d.availability == Availability::Stateless)
                .map(|d| d.module.clone())
                .collect(),
            service_range: service_range.map(str::to_owned),
            published: Vec::new(),
            internet: desired
                .iter()
                .filter(|d| d.internet != Internet::None)
                .map(|d| (d.module.clone(), d.internet))
                .collect(),
            resolvers: desired
                .iter()
                .filter(|d| d.dns_resolver)
                .map(|d| d.module.clone())
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
            .chain(&self.modules)
        {
            if !is_valid_name(n) {
                return Err(format!("invalid name {n:?}"));
            }
        }
        for c in &self.connections {
            c.validate()?;
            if !self.modules.contains(&c.from) || !self.modules.contains(&c.to) {
                return Err(format!(
                    "connection {} → {} names a module outside the application",
                    c.from, c.to
                ));
            }
        }
        if let Some(r) = &self.service_range {
            service_addresses(r)?;
        }
        for p in &self.published {
            p.validate()?;
            if !self.modules.contains(&p.module) {
                return Err(format!("published {} is not in the application", p.module));
            }
        }
        for r in &self.resolvers {
            if !is_valid_name(r) || !self.modules.contains(r) {
                return Err(format!("resolver {r} is not in the application"));
            }
        }
        Ok(())
    }

    /// The ports a service address of `module` listens on: those other modules connect
    /// to and those published.
    pub fn ports_of(&self, module: &str) -> Vec<(String, u16)> {
        let mut ports = service_ports(module, &self.connections);
        ports.extend(
            self.published
                .iter()
                .filter(|p| p.module == module)
                .map(|p| (p.transport.clone(), p.port)),
        );
        ports.sort();
        ports.dedup();
        ports
    }

    /// Modules that get a service address: stateless ones reached on at least one port,
    /// and every published one (outside clients need one stable address, even for a single
    /// instance).
    pub fn services(&self) -> Vec<String> {
        self.modules
            .iter()
            .filter(|a| {
                self.published.iter().any(|p| &p.module == *a)
                    || self.stateless.contains(a) && !service_ports(a, &self.connections).is_empty()
            })
            .cloned()
            .collect()
    }
}

/// Host addresses of one or more routed ranges, comma-separated, lowest first in each range.
/// IPv4: `a.b.c.d/len` with 16 ≤ len ≤ 30, without the network and broadcast addresses.
/// IPv6: `addr/len` with 112 ≤ len ≤ 126, without the subnet address (no broadcast).
pub fn service_addresses(range: &str) -> Result<Vec<IpAddr>, String> {
    let parts: Vec<&str> = range
        .split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    if parts.is_empty() {
        return Err(format!(
            "service range {range:?}: expected an IPv4 range like 198.18.100.0/28 (/16 to /30) or an IPv6 range like fd42::/124 (/112 to /126)"
        ));
    }
    let mut out = Vec::new();
    for p in parts {
        out.extend(one_service_range(p)?);
    }
    Ok(out)
}

fn one_service_range(range: &str) -> Result<Vec<IpAddr>, String> {
    let v4 = || {
        format!("service range {range:?}: expected an IPv4 range like 198.18.100.0/28 (/16 to /30)")
    };
    let v6 = || {
        format!("service range {range:?}: expected an IPv6 range like fd42::/124 (/112 to /126)")
    };
    let (addr, len) = range.split_once('/').ok_or_else(v4)?;
    let len: u32 = len.parse().map_err(|_| v4())?;
    if let Ok(addr) = addr.parse::<Ipv4Addr>() {
        if !(16..=30).contains(&len) {
            return Err(v4());
        }
        let mask = u32::MAX << (32 - len);
        let network = u32::from(addr) & mask;
        if network != u32::from(addr) {
            return Err(format!(
                "service range {range:?}: host bits are set (the range starts at {})",
                Ipv4Addr::from(network)
            ));
        }
        return Ok((network + 1..network + (1 << (32 - len)) - 1)
            .map(|n| IpAddr::V4(Ipv4Addr::from(n)))
            .collect());
    }
    let addr: Ipv6Addr = addr.parse().map_err(|_| v6())?;
    if !(112..=126).contains(&len) {
        return Err(v6());
    }
    let bits = 128 - len;
    let network = u128::from(addr) & (u128::MAX << bits);
    if network != u128::from(addr) {
        return Err(format!(
            "service range {range:?}: host bits are set (the range starts at {})",
            Ipv6Addr::from(network)
        ));
    }
    Ok((network + 1..network + (1u128 << bits))
        .map(|n| IpAddr::V6(Ipv6Addr::from(n)))
        .collect())
}

/// The ports `module` is reached on, as `(transport, port)`, sorted. Connections without a
/// well-known port allow any port and cannot be balanced, so they are left out.
pub fn service_ports(module: &str, connections: &[Connection]) -> Vec<(String, u16)> {
    let mut ports: Vec<(String, u16)> = connections
        .iter()
        .filter(|c| c.to == module)
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
    pub address: IpAddr,
    /// `<instance> <instance uuid>`: a recreated replica changes it even at the same address,
    /// which makes Incus rebind the health check (docs/development/TEST-CLUSTER.md).
    pub description: String,
}

/// A module's service address: an OVN load balancer on the application's network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadBalancer {
    pub listen_address: IpAddr,
    pub module: String,
    pub backends: Vec<Backend>,
    pub ports: Vec<(String, u16)>,
}

/// DNS between every other module and the resolver, on UDP and TCP 53. Both sides are
/// written: an allow on only one side is not enough when egress and ingress are combined
/// with OR and the other side's default is reject. No resolver: no rules.
fn dns_rules(
    application: &str,
    module: &str,
    resolvers: &[String],
    modules: &[String],
) -> (Vec<AclRule>, Vec<AclRule>) {
    if resolvers.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let set = |name: &str| format!("${}", acl_name(application, name));
    let rule = |source: String, destination: String, protocol: &str| AclRule {
        action: "allow".into(),
        source,
        destination,
        protocol: protocol.into(),
        destination_port: "53".into(),
        state: "enabled".into(),
        description: "dns".into(),
    };
    let mut ingress = Vec::new();
    let mut egress = Vec::new();
    if resolvers.iter().any(|r| r == module) {
        let mut others: Vec<&String> = modules.iter().filter(|m| !resolvers.contains(m)).collect();
        others.sort();
        for other in others {
            for protocol in ["tcp", "udp"] {
                ingress.push(rule(set(other), String::new(), protocol));
            }
        }
    } else {
        for resolver in resolvers {
            for protocol in ["tcp", "udp"] {
                egress.push(rule(String::new(), set(resolver), protocol));
            }
        }
    }
    (ingress, egress)
}

/// One module's ACL as Apelora intends it: exactly the rules `network.sync` writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntendedAcl {
    pub module: String,
    /// The ACL's (and the address set's) name.
    pub acl: String,
    pub ingress: Vec<AclRule>,
    pub egress: Vec<AclRule>,
}

impl SyncRequest {
    /// Every module's ACL rules: the connections (both sides), published ports and internet
    /// access. `network.sync` writes these and the Console compares them with Incus.
    pub fn intended_acls(&self) -> Vec<IntendedAcl> {
        let app = self.application.as_str();
        self.modules
            .iter()
            .map(|a| {
                let (mut ingress, mut egress) = acl_rules(app, a, &self.connections);
                let (dns_in, dns_out) = dns_rules(app, a, &self.resolvers, &self.modules);
                ingress.extend(dns_in);
                egress.extend(dns_out);
                ingress.extend(published_rules(a, &self.published));
                egress.extend(internet_rules(
                    self.internet.get(a).copied().unwrap_or_default(),
                ));
                IntendedAcl {
                    module: a.clone(),
                    acl: acl_name(app, a),
                    ingress,
                    egress,
                }
            })
            .collect()
    }
}

/// How the rules Incus holds differ from the intended ones: `(missing, unexpected)`. Order does
/// not matter; Incus may report rule fields Apelora never writes, which are not compared.
pub fn rule_drift(intended: &[AclRule], observed: &[AclRule]) -> (Vec<AclRule>, Vec<AclRule>) {
    let key = |r: &AclRule| {
        (
            r.action.clone(),
            r.source.clone(),
            r.destination.clone(),
            r.protocol.clone(),
            r.destination_port.clone(),
            r.state.clone(),
        )
    };
    let mut rest: Vec<&AclRule> = observed.iter().collect();
    let mut missing = Vec::new();
    for r in intended {
        match rest.iter().position(|o| key(o) == key(r)) {
            Some(i) => {
                rest.remove(i);
            }
            None => missing.push(r.clone()),
        }
    }
    (missing, rest.into_iter().cloned().collect())
}

/// Whether the IPv4 or IPv6 range `inner` lies inside `outer` of the same family.
pub fn ip_range_within(inner: &str, outer: &str) -> bool {
    ipv4_range_within(inner, outer) || ipv6_range_within(inner, outer)
}

/// Whether the IPv6 range `inner` (`addr/len`) lies inside `outer`. `false` when either
/// is not an IPv6 range.
pub fn ipv6_range_within(inner: &str, outer: &str) -> bool {
    fn parse(r: &str) -> Option<(u128, u32)> {
        let (a, l) = r.trim().split_once('/')?;
        let a: Ipv6Addr = a.parse().ok()?;
        let l: u32 = l.parse().ok()?;
        if l > 128 {
            return None;
        }
        let mask = if l == 0 { 0 } else { u128::MAX << (128 - l) };
        Some((u128::from(a) & mask, l))
    }
    match (parse(inner), parse(outer)) {
        (Some((i, il)), Some((o, ol))) => {
            let mask = if ol == 0 { 0 } else { u128::MAX << (128 - ol) };
            il >= ol && i & mask == o
        }
        _ => false,
    }
}

/// Whether the IPv4 range `inner` (`a.b.c.d/len`) lies inside `outer`. `false` when either
/// is not an IPv4 range.
pub fn ipv4_range_within(inner: &str, outer: &str) -> bool {
    fn parse(r: &str) -> Option<(u32, u32)> {
        let (a, l) = r.trim().split_once('/')?;
        let a: Ipv4Addr = a.parse().ok()?;
        let l: u32 = l.parse().ok()?;
        if l > 32 {
            return None;
        }
        let mask = if l == 0 { 0 } else { u32::MAX << (32 - l) };
        Some((u32::from(a) & mask, l))
    }
    match (parse(inner), parse(outer)) {
        (Some((i, il)), Some((o, ol))) => {
            let mask = if ol == 0 { 0 } else { u32::MAX << (32 - ol) };
            il >= ol && i & mask == o
        }
        _ => false,
    }
}

/// Incus 7.0 refuses every load balancer change in the cluster while a member it must notify
/// cannot be reached (`peer node <address> is down`), so a service address keeps sending to the
/// replicas it had. Returns that member's address (`host:port`) when `error` is this refusal.
pub fn offline_member_blocking(error: &str) -> Option<&str> {
    let (_, rest) = error.split_once("peer node ")?;
    let (address, _) = rest.split_once(" is down")?;
    (!address.is_empty() && !address.contains(char::is_whitespace)).then_some(address)
}

/// The member Incus reaches at `address` (`host:port`, as in [`offline_member_blocking`]).
pub fn member_at<'a>(
    members: &'a [crate::ClusterMember],
    address: &str,
) -> Option<&'a crate::ClusterMember> {
    members.iter().find(|m| {
        let url = m.url.trim_end_matches('/');
        url.strip_prefix("https://").unwrap_or(url) == address
    })
}

/// How [`traffic_blocked_reason`] begins, so a recorded job error can be recognised.
pub const TRAFFIC_BLOCKED: &str = "Traffic cannot move to replacement replicas";

/// What an operator reads while [`offline_member_blocking`] holds; `member` is its name when known.
pub fn traffic_blocked_reason(member: &str) -> String {
    format!(
        "{TRAFFIC_BLOCKED}: Incus refuses load balancer changes while {member} is unreachable. Bring it back, or remove it from the cluster (runbook: Member failure)."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_because_a_member_is_unreachable_names_that_member() {
        let e = "Incus API error 500: PUT /1.0/networks/fo/load-balancers/198.18.100.1?project=apelora: Failed updating load balancer: peer node 192.168.104.3:8443 is down";
        assert_eq!(offline_member_blocking(e), Some("192.168.104.3:8443"));
        assert_eq!(
            offline_member_blocking("Incus API error 500: peer node [fd00::3]:8443 is down"),
            Some("[fd00::3]:8443")
        );
        for other in [
            "Incus API error 404: not found",
            "peer node  is down",
            "peer node 10.0.0.1:8443 is slow",
            "",
        ] {
            assert_eq!(offline_member_blocking(other), None, "{other}");
        }
        let member = |name: &str, url: &str| crate::ClusterMember {
            name: name.into(),
            url: url.into(),
            roles: vec![],
            status: "Online".into(),
            message: String::new(),
            architecture: "aarch64".into(),
            failure_domain: "default".into(),
            database: true,
            groups: vec![],
            scheduler: "all".into(),
        };
        let members = [
            member("test-1", "https://192.168.104.2:8443"),
            member("test-2", "https://192.168.104.3:8443/"),
        ];
        assert_eq!(
            member_at(&members, "192.168.104.3:8443").map(|m| m.name.as_str()),
            Some("test-2")
        );
        assert!(member_at(&members, "192.168.104.9:8443").is_none());
        assert!(traffic_blocked_reason("test-2").contains("while test-2 is unreachable"));
    }

    #[test]
    fn intended_acls_are_what_sync_writes_and_drift_ignores_order() {
        let s = SyncRequest {
            project: "apelora".into(),
            application: "shop".into(),
            uplink: "br0".into(),
            modules: vec!["web".into(), "db".into()],
            connections: vec![c("web", "db", Some(5432))],
            replicas: BTreeMap::new(),
            stateless: vec![],
            service_range: None,
            published: vec![Published {
                module: "web".into(),
                transport: "tcp".into(),
                port: 443,
            }],
            internet: BTreeMap::from([("web".to_owned(), Internet::Web)]),
            resolvers: vec![],
        };
        let acls = s.intended_acls();
        let web = acls.iter().find(|a| a.module == "web").expect("web");
        assert_eq!(web.acl, "shop-web");
        assert_eq!(web.egress.len(), 1 + 3, "to db, plus web internet access");
        assert_eq!(web.ingress.len(), 1, "published 443");
        let db = acls.iter().find(|a| a.module == "db").expect("db");
        assert_eq!(db.ingress[0].source, "$shop-web");
        assert_eq!(db.ingress[0].destination_port, "5432");
        let mut reversed = web.egress.clone();
        reversed.reverse();
        assert_eq!(rule_drift(&web.egress, &reversed), (vec![], vec![]));
        let mut changed = web.egress.clone();
        changed[0].destination_port = "5433".into();
        let (missing, extra) = rule_drift(&web.egress, &changed);
        assert_eq!((missing.len(), extra.len()), (1, 1));
        assert_eq!(missing[0].destination_port, "5432");
        assert_eq!(rule_drift(&web.egress, &[]).0.len(), 4);
    }

    #[test]
    fn a_resolver_is_reachable_on_port_53_from_every_other_module() {
        let mut s = SyncRequest {
            project: "apelora".into(),
            application: "shop".into(),
            uplink: "br0".into(),
            modules: vec!["web".into(), "dns".into()],
            connections: vec![],
            replicas: BTreeMap::new(),
            stateless: vec![],
            service_range: None,
            published: vec![],
            internet: BTreeMap::new(),
            resolvers: vec!["dns".into()],
        };
        let acls = s.intended_acls();
        let web = acls.iter().find(|a| a.module == "web").expect("web");
        assert_eq!(web.egress.len(), 2, "udp and tcp to the resolver");
        assert_eq!(web.egress[0].destination, "$shop-dns");
        assert_eq!(web.egress[0].destination_port, "53");
        assert!(web.ingress.is_empty());
        let dns = acls.iter().find(|a| a.module == "dns").expect("dns");
        assert_eq!(dns.ingress.len(), 2);
        assert_eq!(dns.ingress[0].source, "$shop-web");
        assert!(
            dns.egress.is_empty(),
            "the resolver does not open other modules"
        );
        s.resolvers.clear();
        assert!(
            s.intended_acls()
                .iter()
                .all(|a| a.ingress.is_empty() && a.egress.is_empty())
        );
        s.resolvers = vec!["missing".into()];
        assert!(s.validate().is_err());
    }

    #[test]
    fn ranges_inside_ranges() {
        assert!(ipv4_range_within("198.18.100.0/28", "198.18.100.0/28"));
        assert!(ipv4_range_within("198.18.100.16/28", "198.18.100.0/24"));
        assert!(!ipv4_range_within("198.18.101.0/28", "198.18.100.0/24"));
        assert!(!ipv4_range_within("198.18.100.0/24", "198.18.100.0/28"));
        assert!(!ipv4_range_within("not a range", "198.18.100.0/24"));
        assert!(ipv6_range_within("fd42::/124", "fd42::/64"));
        assert!(!ipv6_range_within("fd42::/64", "fd42::/124"));
        assert!(ip_range_within("fd42::/124", "fd42::/64"));
        assert!(!ip_range_within("fd42::/124", "198.18.100.0/24"));
    }

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
            "unconnected modules are isolated"
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
        let v6 = service_addresses("fd42::/124").expect("v6");
        assert_eq!(
            v6.first().map(ToString::to_string).as_deref(),
            Some("fd42::1")
        );
        assert_eq!(
            v6.last().map(ToString::to_string).as_deref(),
            Some("fd42::f")
        );
        assert_eq!(v6.len(), 15);
        let both = service_addresses("198.18.100.0/29, fd42::/124").expect("both");
        assert_eq!(both.len(), 21);
        for bad in [
            "198.18.100.0",
            "198.18.100.1/28",
            "198.18.100.0/31",
            "10.0.0.0/8",
            "::1/64",
            "fd42::/64",
            "fd42::1/124",
            "x/28",
        ] {
            assert!(service_addresses(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn only_connected_stateless_modules_get_a_service_with_their_ports() {
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
            modules: ["lb-01", "web-01", "db-01", "app-01", "raw-01"]
                .map(String::from)
                .to_vec(),
            connections: conns,
            replicas: BTreeMap::new(),
            stateless: ["web-01", "lb-01", "raw-01"].map(String::from).to_vec(),
            service_range: Some("198.18.100.0/28".into()),
            published: vec![],
            internet: BTreeMap::new(),
            resolvers: vec![],
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
            module: "db-01".into(),
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
            modules: ["web-01", "db-01"].map(String::from).to_vec(),
            connections: vec![c("web-01", "db-01", Some(5432))],
            replicas: BTreeMap::new(),
            stateless: vec![],
            service_range: Some("198.18.100.0/28".into()),
            published,
            internet: BTreeMap::new(),
            resolvers: vec![],
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
                module: "nope-01".into(),
                transport: "tcp".into(),
                port: 1,
            }],
            ..s.clone()
        };
        assert!(bad.validate().is_err());
        assert!(
            Published {
                module: "db-01".into(),
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

    #[test]
    fn consumers_learn_a_service_address_or_an_instance_name_per_connection() {
        let conns = [
            c("web-01", "db-01", Some(5432)),
            c("web-01", "cache-01", Some(6379)),
            c("lb-01", "web-01", Some(80)),
        ];
        let addresses = BTreeMap::from([("cache-01".to_owned(), "198.18.100.2".to_owned())]);
        let env = discovery_env("shop", "web-01", &conns, &addresses);
        assert_eq!(
            env,
            BTreeMap::from([
                (
                    "APELORA_SERVICE_CACHE_01".to_owned(),
                    "198.18.100.2".to_owned()
                ),
                (
                    "APELORA_SERVICE_NAME_CACHE_01".to_owned(),
                    "cache-01.shop.internal".to_owned()
                ),
                ("APELORA_SERVICE_DB_01".to_owned(), "shop-db-01".to_owned()),
            ])
        );
        assert!(
            discovery_env("shop", "db-01", &conns, &addresses).is_empty(),
            "providers learn nothing"
        );
        let spec = crate::InstanceSpec {
            project: "apelora".into(),
            name: "shop-web-01".into(),
            kind: crate::InstanceKind::Container,
            source: crate::ImageSource {
                protocol: crate::ImageProtocol::Simplestreams,
                server: "https://images.example".into(),
                alias: "debian/13".into(),
                fingerprint: None,
            },
            profiles: vec![],
            config: Default::default(),
            start: true,
            architectures: vec![],
            owner: None,
            network: None,
            volumes: Vec::new(),
            root_pool: None,
        };
        let with = with_discovery(spec.clone(), &env);
        assert_eq!(
            with.config
                .get("environment.APELORA_SERVICE_DB_01")
                .map(String::as_str),
            Some("shop-db-01")
        );
        // A design cannot set Apelora's discovery variables itself.
        let mut forged = spec;
        forged.config.insert(
            "environment.APELORA_SERVICE_DB_01".into(),
            "203.0.113.9".into(),
        );
        assert!(forged.validate_from_caller().is_err());
        assert_eq!(
            with.validate(),
            Ok(()),
            "Apelora's own variables pass the worker's check"
        );
    }
}
