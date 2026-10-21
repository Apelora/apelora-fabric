// SPDX-License-Identifier: AGPL-3.0-only
//! Service names inside an application (ADR 0004, step 4).
//!
//! The address in `APELORA_SERVICE_<MODULE>` stays the contract. Alongside it, a module that has
//! a service address also gets `APELORA_SERVICE_NAME_<MODULE>` = `<module>.<application>.internal`.
//! The worker publishes those names on the application's network (`user.apelora.dns`, and
//! `dns.domain` so instance names resolve under the same domain). OVN's own DNS still answers
//! only instance names; these records are what a resolver for the service addresses reads.

use std::net::Ipv4Addr;

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
    pub address: Ipv4Addr,
}

/// Records for the modules that have a service address, sorted by name.
pub fn records(application: &str, services: &[(String, String)]) -> Vec<DnsRecord> {
    let mut out = Vec::new();
    for (module, address) in services {
        let Ok(address) = address.parse::<Ipv4Addr>() else {
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

fn write_name(out: &mut Vec<u8>, name: &str) {
    for label in name.split('.').filter(|l| !l.is_empty()) {
        let bytes = label.as_bytes();
        let len = bytes.len().min(63);
        out.push(len as u8);
        out.extend(&bytes[..len]);
    }
    out.push(0);
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
        assert_eq!(got[0].address, Ipv4Addr::new(198, 18, 100, 1));
        assert_eq!(records_text(&got), "web-01.shop.internal 198.18.100.1\n");
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
}
