// SPDX-License-Identifier: AGPL-3.0-only
//! Service names inside an application (ADR 0004, step 4).
//!
//! The address in `APELORA_SERVICE_<MODULE>` stays the contract. Alongside it, a module that has
//! a service address also gets `APELORA_SERVICE_NAME_<MODULE>` = `<module>.<application>.internal`.
//! The worker publishes those names on the application's network (`user.apelora.dns`,
//! `dns.domain`, and a search list that still includes `incus`). OVN's own DNS still answers
//! only instance names. An optional resolver module (ADR 0004 step 4) reads these records and
//! answers them inside the application; it forwards every other name, and the environment
//! variable stays the contract.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// `<module>.<application>.internal`, the name the compiler already gives a service.
pub fn service_name(module: &str, application: &str) -> String {
    format!("{module}.{application}.internal")
}

/// `APELORA_SERVICE_NAME_<MODULE>`, upper case, `-` as `_`.
pub fn name_env(module: &str) -> String {
    format!(
        "APELORA_SERVICE_NAME_{}",
        module.to_ascii_uppercase().replace('-', "_")
    )
}

/// One name and the service address it stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnsRecord {
    pub name: String,
    pub address: IpAddr,
}

/// Records for the modules that have a service address, sorted by name.
pub fn records(application: &str, services: &[(String, String)]) -> Vec<DnsRecord> {
    let mut out = Vec::new();
    for (module, address) in services {
        let Ok(address) = address.parse::<IpAddr>() else {
            continue;
        };
        out.push(DnsRecord {
            name: service_name(module, application),
            address,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out.dedup_by(|a, b| a.name == b.name);
    out
}

/// The text stored on the network (`user.apelora.dns`): one `name address` per line.
pub fn records_text(records: &[DnsRecord]) -> String {
    let mut lines: Vec<String> = records
        .iter()
        .map(|r| format!("{} {}", r.name, r.address))
        .collect();
    lines.sort();
    let mut text = lines.join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    text
}

/// A DNS response (RFC 1035) answering `name` with one A record. The id is copied from the
/// query so a resolver can send it back. TTL is 30 seconds: the worker rewrites the records
/// whenever the service addresses change.
pub fn a_response(id: u16, name: &str, address: Ipv4Addr) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend(id.to_be_bytes());
    out.extend([0x81, 0x80]); // response, no error
    out.extend([0x00, 0x01]); // one question
    out.extend([0x00, 0x01]); // one answer
    out.extend([0x00, 0x00, 0x00, 0x00]); // no authority, no extra
    write_name(&mut out, name);
    out.extend([0x00, 0x01, 0x00, 0x01]); // type A, class IN
    let question_end = out.len();
    // The answer's name is a pointer at the question's name (offset 12).
    out.extend([0xC0, 0x0C]);
    out.extend([0x00, 0x01, 0x00, 0x01]); // type A, class IN
    out.extend(30u32.to_be_bytes());
    out.extend(4u16.to_be_bytes());
    out.extend(address.octets());
    debug_assert!(question_end > 12);
    out
}

/// A DNS response answering `name` with one AAAA record (same layout as [`a_response`]).
pub fn aaaa_response(id: u16, name: &str, address: Ipv6Addr) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend(id.to_be_bytes());
    out.extend([0x81, 0x80]);
    out.extend([0x00, 0x01]);
    out.extend([0x00, 0x01]);
    out.extend([0x00, 0x00, 0x00, 0x00]);
    write_name(&mut out, name);
    out.extend([0x00, 0x1c, 0x00, 0x01]); // type AAAA, class IN
    out.extend([0xC0, 0x0C]);
    out.extend([0x00, 0x1c, 0x00, 0x01]);
    out.extend(30u32.to_be_bytes());
    out.extend(16u16.to_be_bytes());
    out.extend(address.octets());
    out
}

fn write_name(out: &mut Vec<u8>, name: &str) {
    for label in name.split('.').filter(|l| !l.is_empty()) {
        let bytes = label.as_bytes();
        let len = bytes.len().min(63);
        out.push(len as u8);
        out.extend(&bytes[..len]);
    }
    out.push(0);
}

/// One question from a DNS query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    pub id: u16,
    /// The looked-up name, lower case, without a trailing dot.
    pub name: String,
    /// The question type (1 is A, 28 is AAAA).
    pub qtype: u16,
}

/// What the resolver should send back for one query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// A complete response (an answer, or an empty NOERROR when the name is ours but the
    /// type is not).
    Packet(Vec<u8>),
    /// The name is not one of ours: ask the network's previous resolvers.
    Forward,
}

/// `user.apelora.dns` back into records. Lines that are not `name address` are skipped.
pub fn parse_records(text: &str) -> Vec<DnsRecord> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let (Some(name), Some(address)) = (parts.next(), parts.next()) else {
            continue;
        };
        if parts.next().is_some() || !dns_name(name) {
            continue;
        }
        let Ok(address) = address.parse::<IpAddr>() else {
            continue;
        };
        out.push(DnsRecord {
            name: name.to_ascii_lowercase(),
            address,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out.dedup_by(|a, b| a.name == b.name);
    out
}

/// Nameserver addresses in a `resolv.conf` (comments and anything else are ignored).
pub fn resolv_nameservers(text: &str) -> Vec<IpAddr> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        let mut parts = line.split_whitespace();
        if !parts.next().is_some_and(|w| w == "nameserver") {
            continue;
        }
        let Some(addr) = parts.next().and_then(|a| a.parse::<IpAddr>().ok()) else {
            continue;
        };
        if !out.contains(&addr) {
            out.push(addr);
        }
    }
    out
}

/// Where the resolver forwards names it does not hold. Its own addresses are removed so a
/// renewed `resolv.conf` cannot point it at itself. Loopback is removed too: Debian's stub
/// listener is `127.0.0.53`, and once this process holds port 53 that address is itself.
/// With nothing left, `gateway` is the fallback (the application network's router, which
/// answers instance names).
pub fn forwarders(resolv_conf: &str, own: &[IpAddr], gateway: Option<IpAddr>) -> Vec<IpAddr> {
    let mut out = resolv_nameservers(resolv_conf);
    out.retain(|ip| !own.contains(ip) && !ip.is_loopback());
    if out.is_empty()
        && let Some(gateway) = gateway
        && !own.contains(&gateway)
    {
        out.push(gateway);
    }
    out
}

/// Comma-separated addresses, as stored on the network and advertised to DHCP.
pub fn address_list(addresses: &[IpAddr]) -> String {
    let mut out = String::new();
    for ip in addresses {
        if !out.is_empty() {
            out.push(',');
        }
        out.push_str(&ip.to_string());
    }
    out
}

/// The inverse of [`address_list`]: commas, spaces and newlines.
pub fn parse_address_list(text: &str) -> Vec<IpAddr> {
    let mut out = Vec::new();
    for part in text.split(|c: char| c == ',' || c.is_whitespace()) {
        if part.is_empty() {
            continue;
        }
        if let Ok(ip) = part.parse::<IpAddr>()
            && !out.contains(&ip)
        {
            out.push(ip);
        }
    }
    out
}

/// Answers `query` from `records`, or says to forward it. `None` when the packet is not a
/// single question we can read.
pub fn decide(query: &[u8], records: &[DnsRecord]) -> Option<Decision> {
    let q = parse_query(query)?;
    let Some(record) = records
        .iter()
        .find(|r| r.name.eq_ignore_ascii_case(&q.name))
    else {
        return Some(Decision::Forward);
    };
    let packet = match (q.qtype, record.address) {
        (1, IpAddr::V4(address)) => a_response(q.id, &record.name, address),
        (28, IpAddr::V6(address)) => aaaa_response(q.id, &record.name, address),
        (255, IpAddr::V4(address)) => a_response(q.id, &record.name, address),
        (255, IpAddr::V6(address)) => aaaa_response(q.id, &record.name, address),
        // The name is ours; this type is not. An empty NOERROR stops the client from asking
        // the next server and getting a different answer for the same name.
        _ => empty_answer(q.id, &record.name, q.qtype),
    };
    Some(Decision::Packet(packet))
}

/// NOERROR with the question and no answers.
fn empty_answer(id: u16, name: &str, qtype: u16) -> Vec<u8> {
    message(id, 0, name, qtype, None)
}

/// SERVFAIL, used when every forwarder failed. The question is not repeated: the client
/// matches the id.
pub fn servfail(id: u16) -> Vec<u8> {
    let mut out = Vec::with_capacity(12);
    out.extend(id.to_be_bytes());
    out.extend([0x81, 0x82]); // response, recursion available, SERVFAIL
    out.extend([0, 0, 0, 0, 0, 0, 0, 0]);
    out
}

fn message(id: u16, rcode: u8, name: &str, qtype: u16, answer: Option<&[u8]>) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend(id.to_be_bytes());
    out.extend([0x81, 0x80 | rcode]);
    out.extend([0x00, 0x01]);
    out.extend(if answer.is_some() {
        [0x00, 0x01]
    } else {
        [0x00, 0x00]
    });
    out.extend([0x00, 0x00, 0x00, 0x00]);
    write_name(&mut out, name);
    out.extend(qtype.to_be_bytes());
    out.extend([0x00, 0x01]); // class IN
    if let Some(answer) = answer {
        out.extend(answer);
    }
    out
}

fn parse_query(packet: &[u8]) -> Option<Query> {
    if packet.len() < 12 {
        return None;
    }
    let id = u16::from_be_bytes([packet[0], packet[1]]);
    // A response, or more than one question, is not something we answer.
    if packet[2] & 0x80 != 0 || u16::from_be_bytes([packet[4], packet[5]]) != 1 {
        return None;
    }
    let (name, at) = read_name(packet, 12)?;
    if at + 4 > packet.len() {
        return None;
    }
    let qtype = u16::from_be_bytes([packet[at], packet[at + 1]]);
    let qclass = u16::from_be_bytes([packet[at + 2], packet[at + 3]]);
    // IN, or ANY class.
    if qclass != 1 && qclass != 255 {
        return None;
    }
    if name.is_empty() {
        return None;
    }
    Some(Query { id, name, qtype })
}

/// Reads a domain name at `offset`. Returns the name and the index just after it in the
/// original message (a compression pointer counts as two bytes there).
fn read_name(packet: &[u8], offset: usize) -> Option<(String, usize)> {
    let mut labels = Vec::new();
    let mut pos = offset;
    let mut end = None;
    let mut hops = 0;
    loop {
        if hops > 16 || pos >= packet.len() {
            return None;
        }
        let len = packet[pos];
        if len == 0 {
            if end.is_none() {
                end = Some(pos + 1);
            }
            break;
        }
        if len & 0xC0 == 0xC0 {
            if pos + 1 >= packet.len() {
                return None;
            }
            if end.is_none() {
                end = Some(pos + 2);
            }
            pos = (usize::from(len & 0x3F) << 8) | usize::from(packet[pos + 1]);
            hops += 1;
            continue;
        }
        if len & 0xC0 != 0 || len > 63 {
            return None;
        }
        let start = pos + 1;
        let stop = start + usize::from(len);
        if stop > packet.len() {
            return None;
        }
        let label = std::str::from_utf8(&packet[start..stop]).ok()?;
        if !label
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return None;
        }
        labels.push(label.to_ascii_lowercase());
        pos = stop;
    }
    Some((labels.join("."), end?))
}

fn dns_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 253
        && name.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_the_module_and_the_application() {
        assert_eq!(service_name("web-01", "shop"), "web-01.shop.internal");
        assert_eq!(name_env("web-01"), "APELORA_SERVICE_NAME_WEB_01");
    }

    #[test]
    fn records_keep_addresses_and_drop_what_is_not_an_address() {
        let got = records(
            "shop",
            &[
                ("db-01".into(), "shop-db-01".into()),
                ("web-01".into(), "198.18.100.1".into()),
            ],
        );
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "web-01.shop.internal");
        assert_eq!(got[0].address, IpAddr::V4(Ipv4Addr::new(198, 18, 100, 1)));
        assert_eq!(records_text(&got), "web-01.shop.internal 198.18.100.1\n");
        let v6 = records("shop", &[("web-01".into(), "fd42::1".into())]);
        assert_eq!(v6[0].address, "fd42::1".parse::<IpAddr>().expect("v6"));
    }

    #[test]
    fn an_a_response_names_the_address_and_keeps_the_query_id() {
        let packet = a_response(
            0x1234,
            "web-01.shop.internal",
            Ipv4Addr::new(198, 18, 100, 1),
        );
        assert_eq!(&packet[0..2], &0x1234u16.to_be_bytes());
        assert_eq!(&packet[2..4], &[0x81, 0x80]);
        assert_eq!(
            *packet.last_chunk::<4>().expect("address"),
            [198, 18, 100, 1]
        );
        // The question's first label is web-01.
        assert_eq!(packet[12], 6);
        assert_eq!(&packet[13..19], b"web-01");
    }

    #[test]
    fn an_aaaa_response_carries_sixteen_bytes() {
        let address: Ipv6Addr = "fd42::1".parse().expect("v6");
        let packet = aaaa_response(0x1234, "web-01.shop.internal", address);
        assert_eq!(&packet[0..2], &0x1234u16.to_be_bytes());
        assert_eq!(
            packet[packet.len() - 16..],
            address.octets(),
            "AAAA rdata is the address"
        );
        assert_eq!(
            &packet[packet.len() - 18..packet.len() - 16],
            &16u16.to_be_bytes()
        );
    }

    fn query(name: &str, qtype: u16) -> Vec<u8> {
        let mut out = vec![0x12, 0x34, 0x01, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0];
        write_name(&mut out, name);
        out.extend(qtype.to_be_bytes());
        out.extend([0x00, 0x01]);
        out
    }

    #[test]
    fn a_query_for_a_published_name_is_answered_and_anything_else_is_forwarded() {
        let records = parse_records("web-01.shop.internal 198.18.100.1\nnot-an-address\n");
        assert_eq!(records.len(), 1);
        let answer = decide(&query("web-01.shop.internal", 1), &records).expect("query");
        let Decision::Packet(packet) = answer else {
            panic!("the service name is ours");
        };
        assert_eq!(&packet[0..2], &[0x12, 0x34]);
        assert_eq!(
            *packet.last_chunk::<4>().expect("address"),
            [198, 18, 100, 1]
        );
        assert!(matches!(
            decide(&query("other.shop.internal", 1), &records),
            Some(Decision::Forward)
        ));
        let nodata = decide(&query("web-01.shop.internal", 28), &records).expect("aaaa");
        let Decision::Packet(packet) = nodata else {
            panic!("the name is ours, the type is not");
        };
        assert_eq!(&packet[2..4], &[0x81, 0x80], "NOERROR");
        assert_eq!(&packet[6..8], &[0, 0], "no answers");
        assert!(decide(&[0, 1, 2], &records).is_none());
    }

    #[test]
    fn forwarders_skip_the_resolver_itself_and_fall_back_to_the_gateway() {
        let own: IpAddr = "10.1.0.8".parse().expect("own");
        let gateway: IpAddr = "10.1.0.1".parse().expect("gw");
        let conf = "# generated\nnameserver 10.1.0.1\nnameserver 10.1.0.8\nnameserver 192.0.2.53\n";
        assert_eq!(
            forwarders(conf, &[own], Some(gateway)),
            vec![gateway, "192.0.2.53".parse().expect("up")]
        );
        assert_eq!(
            forwarders("nameserver 10.1.0.8\n", &[own], Some(gateway)),
            vec![gateway]
        );
        // The stub listener would loop once this process is the nameserver.
        assert_eq!(
            forwarders("nameserver 127.0.0.53\n", &[own], Some(gateway)),
            vec![gateway]
        );
        assert_eq!(
            parse_address_list("192.0.2.53, 10.1.0.1\n"),
            vec![
                "192.0.2.53".parse::<IpAddr>().expect("a"),
                "10.1.0.1".parse::<IpAddr>().expect("b")
            ]
        );
        assert_eq!(servfail(0x1234)[3] & 0x0f, 2);
    }
}
