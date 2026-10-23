// SPDX-License-Identifier: AGPL-3.0-only
//! Answers an application's service names and forwards every other lookup.
//!
//! Runs inside the resolver module, not on the cluster members. It reads
//! `/etc/apelora-dns.records` (`name address` lines, the same text as `user.apelora.dns`)
//! and `/etc/apelora-dns.upstream` (the resolvers the instance had before it took over)
//! on every query, so a rewrite of either file is picked up without a restart.
//! Listens on UDP and TCP port 53. The environment variables
//! `APELORA_DNS_RECORDS_FILE` and `APELORA_DNS_UPSTREAM_FILE` override the paths.

use std::io::{Read, Write};
use std::net::{IpAddr, TcpListener, UdpSocket};
use std::time::Duration;

use apelora_domain::dns::{self, Decision};

const RECORDS: &str = "/etc/apelora-dns.records";
const UPSTREAM: &str = "/etc/apelora-dns.upstream";

fn main() {
    let records = std::env::var("APELORA_DNS_RECORDS_FILE").unwrap_or_else(|_| RECORDS.to_owned());
    let upstream =
        std::env::var("APELORA_DNS_UPSTREAM_FILE").unwrap_or_else(|_| UPSTREAM.to_owned());
    let udp = match UdpSocket::bind("0.0.0.0:53") {
        Ok(socket) => socket,
        Err(e) => {
            eprintln!("apelora-dns: UDP port 53: {e}");
            std::process::exit(1);
        }
    };
    eprintln!("apelora-dns: serving service names from {records}");
    let records_tcp = records.clone();
    let upstream_tcp = upstream.clone();
    std::thread::spawn(move || serve_tcp(&records_tcp, &upstream_tcp));
    if let Ok(v6) = UdpSocket::bind("[::]:53") {
        let records_v6 = records.clone();
        let upstream_v6 = upstream.clone();
        std::thread::spawn(move || serve_udp(v6, &records_v6, &upstream_v6));
    }
    serve_udp(udp, &records, &upstream);
}

fn serve_udp(socket: UdpSocket, records: &str, upstream: &str) {
    let mut buf = [0u8; 4096];
    loop {
        let Ok((n, from)) = socket.recv_from(&mut buf) else {
            continue;
        };
        if let Some(response) = answer(&buf[..n], records, upstream) {
            let _ = socket.send_to(&response, from);
        }
    }
}

fn serve_tcp(records: &str, upstream: &str) {
    let Ok(listener) = TcpListener::bind("0.0.0.0:53") else {
        eprintln!("apelora-dns: TCP port 53 is not available; UDP still answers");
        return;
    };
    for conn in listener.incoming() {
        let Ok(mut stream) = conn else {
            continue;
        };
        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
        let mut len = [0u8; 2];
        if stream.read_exact(&mut len).is_err() {
            continue;
        }
        let n = usize::from(u16::from_be_bytes(len));
        if !(12..=4096).contains(&n) {
            continue;
        }
        let mut query = vec![0u8; n];
        if stream.read_exact(&mut query).is_err() {
            continue;
        }
        if let Some(response) = answer(&query, records, upstream) {
            let size = u16::try_from(response.len()).unwrap_or(u16::MAX);
            let _ = stream.write_all(&size.to_be_bytes());
            let _ = stream.write_all(&response[..usize::from(size)]);
        }
    }
}

fn answer(query: &[u8], records_path: &str, upstream_path: &str) -> Option<Vec<u8>> {
    let text = std::fs::read_to_string(records_path).unwrap_or_default();
    let records = dns::parse_records(&text);
    match dns::decide(query, &records)? {
        Decision::Packet(packet) => Some(packet),
        Decision::Forward => Some(forward(query, &upstreams(upstream_path))),
    }
}

fn upstreams(path: &str) -> Vec<IpAddr> {
    std::fs::read_to_string(path)
        .map(|text| dns::parse_address_list(&text))
        .unwrap_or_default()
}

fn forward(query: &[u8], upstreams: &[IpAddr]) -> Vec<u8> {
    let id = u16::from_be_bytes([
        query.first().copied().unwrap_or(0),
        query.get(1).copied().unwrap_or(0),
    ]);
    let Ok(socket) = UdpSocket::bind("0.0.0.0:0") else {
        return dns::servfail(id);
    };
    if socket
        .set_read_timeout(Some(Duration::from_secs(2)))
        .is_err()
    {
        return dns::servfail(id);
    }
    let mut buf = [0u8; 4096];
    for upstream in upstreams {
        if socket.send_to(query, (*upstream, 53)).is_err() {
            continue;
        }
        if let Ok((n, _)) = socket.recv_from(&mut buf)
            && n >= 12
        {
            return buf[..n].to_vec();
        }
    }
    dns::servfail(id)
}
