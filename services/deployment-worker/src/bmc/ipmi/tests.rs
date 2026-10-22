// IPMI over the LAN, CONTRACT VERIFIED: known answers for the RAKP codes and session keys
// (computed separately with Python's standard hmac module), the packet framing, and an IPMI BMC on
// loopback UDP driven through the same `Bmc` the supervisor uses.

use std::sync::{Arc, Mutex};

use super::*;
use crate::bmc::{Bmc, Mapping, PowerState};

const UUID: &str = "4c4c4544-0042-4c10-8051-b7c04f435331";
const OTHER_UUID: &str = "11111111-2222-4333-8444-555555555555";
const USER: &str = "apelora-fence";
const PASSWORD: &str = "fence-secret-1";

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn exchange(suite: Suite) -> Exchange<'static> {
    let mut rm = [0u8; 16];
    let mut rc = [0u8; 16];
    let mut guid = [0u8; 16];
    for i in 0..16 {
        rm[i] = i as u8;
        rc[i] = 16 + i as u8;
        guid[i] = 32 + i as u8;
    }
    Exchange {
        suite,
        console_sid: [1, 0, 0, 0],
        bmc_sid: [2, 0, 0, 0],
        rm,
        rc,
        guid,
        role: 0x13,
        user: b"fence",
        password: b"fence-secret-1",
    }
}

#[test]
fn rakp_codes_and_session_keys_match_known_answers_for_both_suites() {
    let x = exchange(Suite::S17);
    let k = x.keys();
    assert_eq!(
        hex(&x.rakp2_code()),
        "ed5e226cbda156435f25380f0156d08b29383b1a93413c94507e2bd00e4fd427"
    );
    assert_eq!(
        hex(&x.rakp3_code()),
        "3455875bc2100931a100a5cf330b9aa996b1f7868a8d7a91a6fbdf56ba4e5972"
    );
    assert_eq!(
        hex(&k.sik),
        "22f651ac918eeb2a0f550d32a511f9cbccfda2218183461a63771e1a9aeaeeee"
    );
    assert_eq!(
        hex(&k.k1),
        "ca78def8d1487f74c2f8110aaafc84dbd1a33dcb17f6026e41deb7eb063cf6b0"
    );
    assert_eq!(hex(&k.aes), "24422f98d740136cf2e17bee56c7a310");
    assert_eq!(hex(&x.rakp4_icv(&k)), "fd15c5bac475d09d1eb94a8a6a213d2e");

    let x = exchange(Suite::S3);
    let k = x.keys();
    assert_eq!(
        hex(&x.rakp2_code()),
        "06319fe37295f61175870a010d2a500cdad5cdf1"
    );
    assert_eq!(
        hex(&x.rakp3_code()),
        "6880e848d175d1b60133f4defcfd8e58b3aaae27"
    );
    assert_eq!(hex(&k.sik), "a2aeee9702e2a8f5c8acc910eab178856236200c");
    assert_eq!(hex(&k.k1), "02eaeebec6d8814a3f9e22b4de7ea45106a2f724");
    assert_eq!(hex(&k.aes), "8ffd36e154ed6126cf9d73b077379139");
    assert_eq!(hex(&x.rakp4_icv(&k)), "927b8b01377da68ce79b8a92");
}

#[test]
fn a_session_packet_opens_only_when_it_is_untouched_authenticated_and_encrypted() {
    for suite in [Suite::S17, Suite::S3] {
        let keys = exchange(suite).keys();
        let msg = request(0x00, 5, 0x01, &[]);
        let p = session_packet(suite, &keys, [9, 0, 0, 0], 7, 0x00, &msg).expect("packet");
        assert_eq!(
            open_session_packet(suite, &keys, &p),
            Some(([9, 0, 0, 0], 7, 0x00, msg.clone()))
        );
        // The integrity code covers whole multiples of four bytes, as the format requires.
        assert_eq!((p.len() - 4 - suite.icv_len()) % 4, 0);
        for i in [5, 10, 20, p.len() - 1] {
            let mut bad = p.clone();
            bad[i] ^= 0x01;
            assert_eq!(open_session_packet(suite, &keys, &bad), None, "byte {i}");
        }
        let other = Exchange {
            password: b"other",
            ..exchange(suite)
        }
        .keys();
        assert_eq!(open_session_packet(suite, &other, &p), None);
        // Without the authenticated and encrypted flags a packet is never read.
        let mut plain = p.clone();
        plain[5] &= 0x3f;
        assert_eq!(open_session_packet(suite, &keys, &plain), None);
    }
}

#[test]
fn the_system_guid_matches_in_each_byte_order_bmcs_use_and_nothing_else() {
    let u = uuid::Uuid::parse_str(UUID).expect("uuid");
    let written = *u.as_bytes();
    let mut reversed = written;
    reversed.reverse();
    for g in [written, reversed, u.to_bytes_le()] {
        assert!(guid_matches(&g, UUID));
        assert!(guid_matches(&g, &UUID.to_uppercase()));
        assert!(!guid_matches(&g, OTHER_UUID));
    }
    assert!(!guid_matches(&[0; 16], UUID));
    assert!(!guid_matches(&written, "not-a-uuid"));
}

/// An IPMI BMC on loopback: one session at a time, RMCP+ only.
struct Fake {
    /// The suites it agrees to; others are refused with "no matching cipher suite".
    suites: Vec<Suite>,
    /// Answer every session request with these algorithms instead.
    answer_with: Option<[u8; 3]>,
    password: String,
    /// Highest role the user may take.
    max_role: u8,
    /// What Get System GUID answers.
    guid: [u8; 16],
    on: bool,
    stays_on: bool,
    /// Chassis control requests (their data byte).
    controls: Vec<u8>,
    /// Suite of each session, and how many were closed.
    sessions: Vec<Suite>,
    closed: usize,
}

type Shared = Arc<Mutex<Fake>>;

struct Live {
    suite: Suite,
    keys: Option<Keys>,
    console_sid: [u8; 4],
    bmc_sid: [u8; 4],
    rm: [u8; 16],
    rc: [u8; 16],
    role: u8,
    user: Vec<u8>,
    seq: u32,
}

fn reply(netfn: u8, rq_seq: u8, cmd: u8, cc: u8, data: &[u8]) -> Vec<u8> {
    let mut m = vec![0x81, (netfn | 1) << 2];
    m.push(checksum(&m));
    let start = m.len();
    m.extend_from_slice(&[0x20, rq_seq << 2, cmd, cc]);
    m.extend_from_slice(data);
    m.push(checksum(&m[start..]));
    m
}

const RAKP_GUID: [u8; 16] = [7; 16];

fn handle(f: &Shared, live: &mut Option<Live>, p: &[u8]) -> Option<Vec<u8>> {
    let mut g = f.lock().expect("fake");
    if let Some((t, b)) = parse_plain(p)
        && p[5] & 0xc0 == 0
    {
        match t {
            OPEN_REQUEST => {
                let tag = b[0];
                let console_sid = [b[4], b[5], b[6], b[7]];
                let proposed = [b[12], b[20], b[28]];
                let (status, algos) = match g.answer_with {
                    Some(a) => (0, a),
                    None => match Suite::of(proposed).filter(|s| g.suites.contains(s)) {
                        Some(_) => (0, proposed),
                        None => (0x11, proposed),
                    },
                };
                let bmc_sid = [0x5a, 0x11, 0, 0];
                let mut r = vec![tag, status, 3, 0];
                r.extend_from_slice(&console_sid);
                if status == 0 {
                    r.extend_from_slice(&bmc_sid);
                    r.extend_from_slice(&[0, 0, 0, 8, algos[0], 0, 0, 0]);
                    r.extend_from_slice(&[1, 0, 0, 8, algos[1], 0, 0, 0]);
                    r.extend_from_slice(&[2, 0, 0, 8, algos[2], 0, 0, 0]);
                    *live = Some(Live {
                        suite: Suite::of(algos).unwrap_or(Suite::S17),
                        keys: None,
                        console_sid,
                        bmc_sid,
                        rm: [0; 16],
                        rc: [0x33; 16],
                        role: 0,
                        user: vec![],
                        seq: 0,
                    });
                }
                Some(plain_packet(OPEN_RESPONSE, &r))
            }
            RAKP1 => {
                let l = live.as_mut()?;
                let tag = b[0];
                l.rm.copy_from_slice(&b[8..24]);
                l.role = b[24];
                l.user = b[28..28 + b[27] as usize].to_vec();
                let mut r = vec![tag, 0, 0, 0];
                r.extend_from_slice(&l.console_sid);
                if l.user != USER.as_bytes() {
                    r[1] = 0x0d;
                    return Some(plain_packet(RAKP2, &r));
                }
                if l.role & 0x0f > g.max_role {
                    r[1] = 0x09;
                    return Some(plain_packet(RAKP2, &r));
                }
                let x = Exchange {
                    suite: l.suite,
                    console_sid: l.console_sid,
                    bmc_sid: l.bmc_sid,
                    rm: l.rm,
                    rc: l.rc,
                    guid: RAKP_GUID,
                    role: l.role,
                    user: &l.user,
                    password: g.password.as_bytes(),
                };
                r.extend_from_slice(&l.rc);
                r.extend_from_slice(&RAKP_GUID);
                r.extend(x.rakp2_code());
                Some(plain_packet(RAKP2, &r))
            }
            RAKP3 => {
                let l = live.as_mut()?;
                let x = Exchange {
                    suite: l.suite,
                    console_sid: l.console_sid,
                    bmc_sid: l.bmc_sid,
                    rm: l.rm,
                    rc: l.rc,
                    guid: RAKP_GUID,
                    role: l.role,
                    user: &l.user,
                    password: g.password.as_bytes(),
                };
                let mut r = vec![b[0], 0, 0, 0];
                r.extend_from_slice(&l.console_sid);
                if b[8..] != x.rakp3_code()[..] {
                    r[1] = 0x0f;
                    return Some(plain_packet(RAKP4, &r));
                }
                let keys = x.keys();
                r.extend(x.rakp4_icv(&keys));
                l.keys = Some(keys);
                g.sessions.push(l.suite);
                Some(plain_packet(RAKP4, &r))
            }
            _ => None,
        }
    } else {
        let l = live.as_mut()?;
        let keys = l.keys.clone()?;
        let (sid, _, _, m) = open_session_packet(l.suite, &keys, p)?;
        if sid != l.bmc_sid {
            return None;
        }
        let (netfn, rq_seq, cmd) = (m[1] >> 2, m[4] >> 2, m[5]);
        let data = &m[6..m.len() - 1];
        let (cc, out): (u8, Vec<u8>) = match (netfn, cmd) {
            (0x06, 0x3b) => (0, vec![data[0]]),
            (0x06, 0x37) => (0, g.guid.to_vec()),
            (0x06, 0x3c) => {
                g.closed += 1;
                (0, vec![])
            }
            (0x00, 0x01) => (0, vec![u8::from(g.on), 0, 0, 0]),
            (0x00, 0x02) => {
                g.controls.push(data[0]);
                if data[0] == 0 && !g.stays_on {
                    g.on = false;
                }
                (0, vec![])
            }
            _ => (0xc1, vec![]),
        };
        l.seq += 1;
        let r = reply(netfn, rq_seq, cmd, cc, &out);
        session_packet(l.suite, &keys, l.console_sid, l.seq, 0x00, &r).ok()
    }
}

async fn fake_bmc(on: bool) -> (u16, Shared) {
    let shared: Shared = Arc::new(Mutex::new(Fake {
        suites: vec![Suite::S17, Suite::S3],
        answer_with: None,
        password: PASSWORD.into(),
        max_role: 4,
        guid: *uuid::Uuid::parse_str(UUID).expect("uuid").as_bytes(),
        on,
        stays_on: false,
        controls: vec![],
        sessions: vec![],
        closed: 0,
    }));
    let sock = UdpSocket::bind("127.0.0.1:0").await.expect("bind");
    let port = sock.local_addr().expect("addr").port();
    let f = shared.clone();
    tokio::spawn(async move {
        let mut buf = vec![0u8; 1024];
        let mut live = None;
        while let Ok((n, from)) = sock.recv_from(&mut buf).await {
            if let Some(out) = handle(&f, &mut live, &buf[..n]) {
                let _ = sock.send_to(&out, from).await;
            }
        }
    });
    (port, shared)
}

fn mapping(port: u16, uuid: &str, secret: Option<&str>) -> Bmc {
    let text = format!(
        r#"{{ "m1": {{ "provider": "ipmi", "host": "127.0.0.1", "port": {port}, "user": "{USER}",
              "secretRef": "APELORA_BMC_SECRET_M1", "uuid": "{uuid}" }} }}"#
    );
    let mut m = Mapping::parse(&text, false).expect("mapping");
    m.settle = Some((Duration::from_millis(300), Duration::from_millis(20)));
    let secret = secret.map(str::to_owned);
    m.secrets = Some(Arc::new(move |k| {
        (k == "APELORA_BMC_SECRET_M1")
            .then(|| secret.clone())
            .flatten()
    }));
    Bmc::Mapped(m)
}

#[tokio::test]
async fn a_server_is_switched_off_over_ipmi_and_the_power_is_read_back_before_it_counts() {
    let (port, fake) = fake_bmc(true).await;
    let b = mapping(port, UUID, Some(PASSWORD));
    assert_eq!(b.provider_of("m1"), "ipmi");
    assert_eq!(
        b.system_of("m1"),
        UUID,
        "the evidence names the GUID, never the address"
    );
    assert_eq!(b.power_state("m1").await, Ok(PowerState::On));
    let r = b.fence("m1").await.expect("fenced");
    assert_eq!(
        (r.provider.as_str(), r.system.as_str(), r.switched_off),
        ("ipmi", UUID, true)
    );
    assert_eq!(b.power_state("m1").await, Ok(PowerState::Off));
    // Already off: nothing is sent again.
    assert!(!b.fence("m1").await.expect("already off").switched_off);
    let g = fake.lock().expect("fake");
    assert_eq!(g.controls, vec![0x00], "one hard power-off, nothing else");
    assert_eq!(
        g.sessions,
        vec![Suite::S17; 4],
        "suite 17 when the BMC offers it"
    );
    assert_eq!(g.closed, 4, "every session is closed");
}

#[tokio::test]
async fn a_bmc_without_suite_17_is_used_with_suite_3_and_one_offering_neither_is_refused() {
    let (port, fake) = fake_bmc(true).await;
    fake.lock().expect("fake").suites = vec![Suite::S3];
    let b = mapping(port, UUID, Some(PASSWORD));
    assert!(b.fence("m1").await.expect("fenced").switched_off);
    assert_eq!(fake.lock().expect("fake").sessions, vec![Suite::S3]);

    let (port, fake) = fake_bmc(true).await;
    fake.lock().expect("fake").suites = vec![];
    let e = mapping(port, UUID, Some(PASSWORD))
        .fence("m1")
        .await
        .expect_err("no suite");
    assert!(e.contains("neither cipher suite 17 nor 3"), "{e}");
    assert!(fake.lock().expect("fake").controls.is_empty());
}

#[tokio::test]
async fn a_bmc_that_answers_without_authentication_or_encryption_is_refused() {
    for algos in [[0, 0, 0], [3, 4, 0], [1, 0, 1], [3, 1, 1]] {
        let (port, fake) = fake_bmc(true).await;
        fake.lock().expect("fake").answer_with = Some(algos);
        let e = mapping(port, UUID, Some(PASSWORD))
            .fence("m1")
            .await
            .expect_err("weak");
        assert!(e.contains("only cipher suites 17 and 3"), "{algos:?}: {e}");
        assert!(fake.lock().expect("fake").controls.is_empty());
    }
}

#[tokio::test]
async fn another_servers_guid_refuses_before_the_power_is_read_or_switched() {
    let (port, fake) = fake_bmc(true).await;
    fake.lock().expect("fake").guid = *uuid::Uuid::parse_str(OTHER_UUID).expect("uuid").as_bytes();
    let b = mapping(port, UUID, Some(PASSWORD));
    let e = b.fence("m1").await.expect_err("wrong server");
    assert!(e.contains("refusing") && e.contains(OTHER_UUID), "{e}");
    assert!(
        b.power_state("m1").await.is_err(),
        "another server's Off is not evidence"
    );
    let g = fake.lock().expect("fake");
    assert!(g.controls.is_empty());
    assert_eq!(g.closed, 2, "the session is closed after the refusal");
}

#[tokio::test]
async fn a_bmc_that_stores_the_guid_little_endian_is_the_same_server() {
    let (port, fake) = fake_bmc(true).await;
    fake.lock().expect("fake").guid = uuid::Uuid::parse_str(UUID).expect("uuid").to_bytes_le();
    assert!(
        mapping(port, UUID, Some(PASSWORD))
            .fence("m1")
            .await
            .expect("fenced")
            .switched_off
    );
}

#[tokio::test]
async fn wrong_credentials_or_role_refuse_without_putting_the_password_in_the_error() {
    let (port, fake) = fake_bmc(true).await;
    let e = mapping(port, UUID, Some("not-the-password"))
        .fence("m1")
        .await
        .expect_err("wrong");
    assert!(e.contains("does not match the password"), "{e}");
    assert!(
        !e.contains("not-the-password") && !e.contains(PASSWORD),
        "{e}"
    );
    let e = mapping(port, UUID, None)
        .fence("m1")
        .await
        .expect_err("no secret");
    assert!(e.contains("APELORA_BMC_SECRET_M1"), "{e}");
    fake.lock().expect("fake").max_role = 2;
    let e = mapping(port, UUID, Some(PASSWORD))
        .fence("m1")
        .await
        .expect_err("role");
    assert!(e.contains("operator"), "{e}");
    assert!(fake.lock().expect("fake").controls.is_empty());
}

#[tokio::test]
async fn an_accepted_power_off_that_leaves_the_server_on_is_not_evidence() {
    let (port, fake) = fake_bmc(true).await;
    fake.lock().expect("fake").stays_on = true;
    let e = mapping(port, UUID, Some(PASSWORD))
        .fence("m1")
        .await
        .expect_err("still on");
    assert!(e.contains("still does not report Off"), "{e}");
    assert_eq!(fake.lock().expect("fake").closed, 1);
}

#[test]
fn the_ipmi_mapping_is_checked_and_every_problem_is_named() {
    let entry = |extra: &str| {
        format!(
            r#"{{ "m1": {{ "provider": "ipmi", "host": "10.0.0.21", "user": "{USER}",
               "secretRef": "APELORA_BMC_SECRET_M1", "uuid": "{UUID}" {extra} }} }}"#
        )
    };
    let ok = |t: &str| Mapping::parse(t, false);
    let m = ok(&entry("")).expect("valid");
    assert!(matches!(
        m.targets["m1"].entry,
        crate::bmc::Entry::Ipmi { port: 623, .. }
    ));
    assert!(ok(&entry("").replace("10.0.0.21", "bmc-21.example.org")).is_ok());
    assert!(ok(&entry("").replace("10.0.0.21", "fd00::21")).is_ok());
    for host in [
        "https://10.0.0.21",
        "10.0.0.21:623",
        "10.0.0.21/x",
        "",
        "a b",
    ] {
        assert!(
            ok(&entry("").replace("10.0.0.21", host))
                .expect_err(host)
                .contains("host"),
            "{host}"
        );
    }
    assert!(
        ok(&entry(r#", "port": 0"#))
            .expect_err("port")
            .contains("port")
    );
    assert!(
        ok(&entry("").replace(USER, "seventeen-chars-x"))
            .expect_err("user")
            .contains("16")
    );
    assert!(ok(&entry("").replace(USER, "has space")).is_err());
    assert!(ok(&entry("").replace("APELORA_BMC_SECRET_M1", "PATH")).is_err());
    assert!(
        ok(&entry("").replace(UUID, "nope"))
            .expect_err("uuid")
            .contains("GUID")
    );
    assert!(
        ok(&entry(r#", "password": "x""#)).is_err(),
        "no password in the mapping"
    );
    assert!(
        ok(&entry(r#", "cipherSuite": 0"#)).is_err(),
        "no way to choose a weaker suite"
    );
}
