// A Redfish-shaped BMC on loopback (CONTRACT VERIFIED: the shapes are the DMTF interface's, not a
// vendor's), the mapping's checks, and the fixed provider the supervisor's tests use.
// Test fixtures use fixed secrets and keys on purpose; this file is left out of code scanning.

use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose, SanType,
};
use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::io::copy_bidirectional;

use super::*;

const UUID: &str = "4c4c4544-0042-4c10-8051-b7c04f435331";
const OTHER_UUID: &str = "11111111-2222-4333-8444-555555555555";

fn b64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in data.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

#[derive(Default)]
struct Fake {
    on: bool,
    uuid: String,
    /// The request is accepted but the power state does not change.
    stays_on: bool,
    /// `ResetType@Redfish.AllowableValues`.
    allowed: Vec<&'static str>,
    /// Two systems listed instead of one.
    two_systems: bool,
    resets: Vec<String>,
    seen_auth: Vec<String>,
}

type Shared = Arc<Mutex<Fake>>;

fn json(v: Value) -> Response {
    ([("content-type", "application/json")], v.to_string()).into_response()
}

fn authed(s: &Shared, h: &HeaderMap) -> bool {
    let want = format!("Basic {}", b64(b"apelora-fence:hunter2-lab-only"));
    let got = h
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    s.lock().expect("fake").seen_auth.push(got.clone());
    got == want
}

async fn systems(State(s): State<Shared>, h: HeaderMap) -> Response {
    if !authed(&s, &h) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let mut members = vec![serde_json::json!({ "@odata.id": "/redfish/v1/Systems/1" })];
    if s.lock().expect("fake").two_systems {
        members.push(serde_json::json!({ "@odata.id": "/redfish/v1/Systems/2" }));
    }
    json(serde_json::json!({ "Members": members }))
}

async fn system(State(s): State<Shared>, h: HeaderMap) -> Response {
    if !authed(&s, &h) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let f = s.lock().expect("fake");
    json(serde_json::json!({
        "UUID": f.uuid,
        "PowerState": if f.on { "On" } else { "Off" },
        "Actions": { "#ComputerSystem.Reset": {
            "target": "/redfish/v1/Systems/1/Actions/ComputerSystem.Reset",
            "ResetType@Redfish.AllowableValues": f.allowed } }
    }))
}

async fn reset(State(s): State<Shared>, h: HeaderMap, body: String) -> Response {
    if !authed(&s, &h) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let mut f = s.lock().expect("fake");
    let kind = serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|v| {
            v.get("ResetType")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_default();
    f.resets.push(kind.clone());
    if kind == "ForceOff" && !f.stays_on {
        f.on = false;
    }
    StatusCode::NO_CONTENT.into_response()
}

/// A CA and a loopback leaf (CA:FALSE, serverAuth, IP SAN). The shipping client trusts the CA
/// file; rustls refuses a CA:TRUE end-entity (LESSONS-LEARNED).
fn certificates() -> (String, rustls::ServerConfig) {
    let ca_key = KeyPair::generate().expect("key");
    let mut ca = CertificateParams::new(Vec::<String>::new()).expect("params");
    ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca.distinguished_name
        .push(DnType::CommonName, "bmc test CA");
    ca.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    let ca_cert = ca.self_signed(&ca_key).expect("CA");
    let issuer = Issuer::new(ca, ca_key);
    let key = KeyPair::generate().expect("key");
    let mut leaf = CertificateParams::new(Vec::<String>::new()).expect("params");
    leaf.subject_alt_names = vec![SanType::IpAddress(IpAddr::V4(Ipv4Addr::LOCALHOST))];
    leaf.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    leaf.is_ca = IsCa::NoCa;
    let leaf_cert = leaf.signed_by(&key, &issuer).expect("leaf");
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("versions")
    .with_no_client_auth()
    .with_single_cert(
        vec![leaf_cert.der().clone()],
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
    )
    .expect("server config");
    (ca_cert.pem(), config)
}

async fn bmc(on: bool) -> (String, Shared, PathBuf) {
    let shared: Shared = Arc::new(Mutex::new(Fake {
        on,
        uuid: UUID.into(),
        allowed: vec!["ForceOff", "GracefulShutdown", "On"],
        ..Default::default()
    }));
    let app = Router::new()
        .route("/redfish/v1/Systems", get(systems))
        .route("/redfish/v1/Systems/1", get(system))
        .route(
            "/redfish/v1/Systems/1/Actions/ComputerSystem.Reset",
            post(reset),
        )
        .with_state(shared.clone());
    let http = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let http_addr = http.local_addr().expect("addr");
    tokio::spawn(async move { axum::serve(http, app).await.ok() });

    let (ca_pem, config) = certificates();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let https = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let https_addr = https.local_addr().expect("addr");
    tokio::spawn(async move {
        while let Ok((tcp, _)) = https.accept().await {
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(tcp).await else {
                    return;
                };
                let Ok(mut backend) = tokio::net::TcpStream::connect(http_addr).await else {
                    return;
                };
                let _ = copy_bidirectional(&mut tls, &mut backend).await;
            });
        }
    });
    let ca = std::env::temp_dir().join(format!(
        "apelora-bmc-ca-{}-{}.pem",
        std::process::id(),
        https_addr.port()
    ));
    std::fs::write(&ca, ca_pem).expect("ca");
    (format!("https://{https_addr}"), shared, ca)
}

fn mapping(url: &str, uuid: &str, secret: Option<&str>, ca: &Path) -> Bmc {
    let text = serde_json::json!({
        "m1": {
            "provider": "redfish",
            "url": url,
            "user": "apelora-fence",
            "secretRef": "APELORA_BMC_SECRET_M1",
            "uuid": uuid,
            "ca": ca,
        }
    })
    .to_string();
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

const PASSWORD: &str = "hunter2-lab-only";

#[tokio::test]
async fn a_server_is_switched_off_and_the_power_state_is_read_back_before_it_counts() {
    let (url, fake, ca) = bmc(true).await;
    let b = mapping(&url, UUID, Some(PASSWORD), &ca);
    assert!(b.covers("m1") && !b.covers("m2"));
    assert_eq!(b.power_state("m1").await, Ok(PowerState::On));
    let r = b.fence("m1").await.expect("fenced");
    assert_eq!(
        (r.provider.as_str(), r.system.as_str(), r.switched_off),
        ("redfish", "/redfish/v1/Systems/1", true)
    );
    assert_eq!(
        fake.lock().expect("fake").resets,
        vec!["ForceOff".to_owned()]
    );
    assert_eq!(b.power_state("m1").await, Ok(PowerState::Off));
    // Already off: nothing is sent again, and the reading says so.
    let again = b.fence("m1").await.expect("already off");
    assert!(!again.switched_off);
    assert_eq!(fake.lock().expect("fake").resets.len(), 1);
}

#[tokio::test]
async fn an_accepted_request_that_leaves_the_server_on_is_not_evidence() {
    let (url, fake, ca) = bmc(true).await;
    fake.lock().expect("fake").stays_on = true;
    let e = mapping(&url, UUID, Some(PASSWORD), &ca)
        .fence("m1")
        .await
        .expect_err("still on");
    assert!(e.contains("still does not report Off"), "{e}");
}

#[tokio::test]
async fn a_bmc_that_reports_another_servers_uuid_is_refused_before_anything_is_sent() {
    let (url, fake, ca) = bmc(true).await;
    fake.lock().expect("fake").uuid = OTHER_UUID.into();
    let b = mapping(&url, UUID, Some(PASSWORD), &ca);
    let e = b.fence("m1").await.expect_err("wrong server");
    assert!(e.contains("refusing") && e.contains(OTHER_UUID), "{e}");
    assert!(
        fake.lock().expect("fake").resets.is_empty(),
        "nothing was sent"
    );
    // Reading the power state is refused too: another server's Off would be false evidence.
    assert!(b.power_state("m1").await.is_err());
}

#[tokio::test]
async fn wrong_or_missing_credentials_refuse_without_putting_the_password_in_the_error() {
    let (url, _, ca) = bmc(true).await;
    let e = mapping(&url, UUID, Some("not-the-password"), &ca)
        .fence("m1")
        .await
        .expect_err("refused");
    assert!(e.contains("refused the credentials"), "{e}");
    assert!(
        !e.contains("not-the-password") && !e.contains(PASSWORD),
        "{e}"
    );
    let e = mapping(&url, UUID, None, &ca)
        .fence("m1")
        .await
        .expect_err("no secret");
    assert!(e.contains("APELORA_BMC_SECRET_M1"), "{e}");
}

#[tokio::test]
async fn a_system_that_does_not_offer_force_off_and_a_bmc_with_several_systems_refuse() {
    let (url, fake, ca) = bmc(true).await;
    fake.lock().expect("fake").allowed = vec!["GracefulShutdown"];
    let e = mapping(&url, UUID, Some(PASSWORD), &ca)
        .fence("m1")
        .await
        .expect_err("no force off");
    assert!(e.contains("does not offer ForceOff"), "{e}");
    assert!(fake.lock().expect("fake").resets.is_empty());
    let (url, fake, ca) = bmc(true).await;
    fake.lock().expect("fake").two_systems = true;
    let e = mapping(&url, UUID, Some(PASSWORD), &ca)
        .fence("m1")
        .await
        .expect_err("two systems");
    assert!(e.contains("several systems"), "{e}");
}

#[test]
fn the_mapping_is_checked_and_every_problem_is_named() {
    let ok = |text: &str| Mapping::parse(text, false);
    let redfish = |extra: &str| {
        format!(
            r#"{{ "m1": {{ "provider": "redfish", "url": "https://10.0.0.11", "user": "u",
               "secretRef": "APELORA_BMC_SECRET_M1", "uuid": "{UUID}" {extra} }} }}"#
        )
    };
    assert_eq!(ok(&redfish("")).expect("valid").targets.len(), 1);
    assert!(ok("[]").is_err() && ok("{").is_err());
    // No plain HTTP, no credentials in the address, no free-text secret names, the UUID is required.
    assert!(
        ok(&redfish("").replace("https://", "http://"))
            .expect_err("http")
            .contains("https")
    );
    assert!(
        ok(&redfish("").replace("10.0.0.11", "u:p@10.0.0.11"))
            .expect_err("credentials")
            .contains("credentials")
    );
    assert!(
        ok(&redfish("").replace("APELORA_BMC_SECRET_M1", "HOME"))
            .expect_err("secret")
            .contains("APELORA_BMC_SECRET_")
    );
    assert!(
        ok(&redfish("").replace(UUID, "not-a-uuid"))
            .expect_err("uuid")
            .contains("hardware UUID")
    );
    assert!(
        ok(&redfish("").replace(&format!(r#", "uuid": "{UUID}""#), "")).is_err(),
        "the UUID cannot be left out"
    );
    assert!(
        ok(&redfish(
            r#", "system": "/redfish/v1/Systems/../Managers/1""#
        ))
        .is_err()
    );
    assert!(
        ok(&redfish(r#", "ca": "/nonexistent/ca.pem""#))
            .expect_err("ca")
            .contains("missing")
    );
    assert!(
        ok(&redfish(r#", "insecure": true"#)).is_err(),
        "there is no insecure mode"
    );
    // The lab provider needs the lab switch, a lab VM name, and never the SAN.
    let lima = |vm: &str| format!(r#"{{ "m4": {{ "provider": "lima", "vm": "{vm}" }} }}"#);
    assert!(
        ok(&lima("apelora-incus-4"))
            .expect_err("lab")
            .contains("APELORA_BMC_ALLOW_LAB")
    );
    assert!(Mapping::parse(&lima("apelora-incus-4"), true).is_ok());
    assert!(Mapping::parse(&lima("apelora-san"), true).is_err());
    assert!(Mapping::parse(&lima("my-laptop"), true).is_err());
    assert!(Mapping::parse(&lima("apelora-x; rm -rf"), true).is_err());
}

#[test]
fn the_settings_come_from_the_environment_and_nothing_secret_is_in_them() {
    let none = |_: &str| None;
    let (b, after) = bmc_from_env(none).expect("off");
    assert!(!b.configured());
    assert_eq!(after, 30);
    for bad in ["19", "3601", "soon"] {
        assert!(
            bmc_from_env(|k| (k == "APELORA_BMC_FENCE_AFTER_SECS").then(|| bad.to_owned()))
                .is_err(),
            "{bad}"
        );
    }
    assert!(
        bmc_from_env(|k| (k == "APELORA_BMC_MAP").then(|| "/nonexistent/map.json".to_owned()))
            .is_err()
    );
    let dir = std::env::temp_dir().join(format!("apelora-bmc-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let file = dir.join("map.json");
    std::fs::write(
        &file,
        r#"{ "m4": { "provider": "lima", "vm": "apelora-incus-4" } }"#,
    )
    .expect("write");
    let env = |k: &str| match k {
        "APELORA_BMC_MAP" => Some(file.display().to_string()),
        "APELORA_BMC_ALLOW_LAB" => Some("1".into()),
        "APELORA_BMC_FENCE_AFTER_SECS" => Some("45".into()),
        _ => None,
    };
    let (b, after) = bmc_from_env(env).expect("configured");
    assert_eq!((b.members(), after), (vec!["m4".to_owned()], 45));
    assert!(!format!("{b:?}").contains("secret"), "{b:?}");
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn the_fixed_provider_records_what_it_was_asked() {
    let fixed = Bmc::Fixed(Arc::new(Mutex::new((
        BTreeMap::from([(
            "m1".to_owned(),
            FixedMember {
                on: true,
                refuse: None,
                stays_on: false,
            },
        )]),
        vec![],
    ))));
    let r = fixed.fence("m1").await.expect("fenced");
    assert!(r.switched_off);
    assert_eq!(fixed.power_state("m1").await, Ok(PowerState::Off));
    assert!(fixed.fence("m2").await.is_err());
}

// ---- the maas provider: the BMC stays in MAAS (CONTRACT VERIFIED against a MAAS-shaped server) ----

#[derive(Default)]
struct FakeMaas {
    on: bool,
    uuid: Option<String>,
    stays_on: bool,
    offs: usize,
}
type SharedMaas = Arc<Mutex<FakeMaas>>;

fn maas_json(f: &FakeMaas) -> Response {
    json(
        serde_json::json!({ "system_id": "abc123", "hostname": "rack-a-1", "status_name": "Deployed",
        "power_state": if f.on { "on" } else { "off" }, "power_type": "redfish", "hardware_uuid": f.uuid }),
    )
}

async fn maas_one(State(s): State<SharedMaas>) -> Response {
    maas_json(&s.lock().expect("fake"))
}

async fn maas_query(State(s): State<SharedMaas>) -> Response {
    json(serde_json::json!({ "state": if s.lock().expect("fake").on { "on" } else { "off" } }))
}

async fn maas_off(State(s): State<SharedMaas>, body: String) -> Response {
    let mut f = s.lock().expect("fake");
    assert_eq!(body, "stop_mode=hard");
    f.offs += 1;
    if !f.stays_on {
        f.on = false;
    }
    maas_json(&f)
}

async fn maas(on: bool, uuid: Option<&str>) -> (Bmc, SharedMaas) {
    let shared: SharedMaas = Arc::new(Mutex::new(FakeMaas {
        on,
        uuid: uuid.map(str::to_owned),
        ..Default::default()
    }));
    let app = Router::new()
        .route("/MAAS/api/2.0/machines/{id}/", get(maas_one))
        .route(
            "/MAAS/api/2.0/machines/{id}/op-query_power_state",
            post(maas_query),
        )
        .route("/MAAS/api/2.0/machines/{id}/op-power_off", post(maas_off))
        .with_state(shared.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move { axum::serve(listener, app).await.ok() });
    let mut m = Mapping::parse(
        &format!(r#"{{ "m1": {{ "provider": "maas", "systemId": "abc123", "uuid": "{UUID}" }} }}"#),
        false,
    )
    .expect("mapping");
    m.maas = Some(apelora_maas_adapter::MaasClient::plain_for_tests(
        &format!("http://{addr}/MAAS"),
        apelora_maas_adapter::ApiKey::parse("c:t:s").expect("key"),
    ));
    m.settle = Some((Duration::from_millis(300), Duration::from_millis(20)));
    (Bmc::Mapped(m), shared)
}

#[tokio::test]
async fn maas_switches_a_server_off_and_the_state_is_read_back_from_maas() {
    let (b, fake) = maas(true, Some(UUID)).await;
    assert_eq!(
        (b.provider_of("m1").as_str(), b.system_of("m1").as_str()),
        ("maas", "abc123")
    );
    assert_eq!(b.power_state("m1").await, Ok(PowerState::On));
    let r = b.fence("m1").await.expect("fenced");
    assert_eq!(
        (r.provider.as_str(), r.system.as_str(), r.switched_off),
        ("maas", "abc123", true)
    );
    assert_eq!(fake.lock().expect("fake").offs, 1);
    let again = b.fence("m1").await.expect("already off");
    assert!(!again.switched_off);
    assert_eq!(
        fake.lock().expect("fake").offs,
        1,
        "nothing is sent to a server that is off"
    );
}

#[tokio::test]
async fn maas_refuses_another_servers_uuid_and_a_machine_it_has_not_commissioned() {
    let (b, fake) = maas(true, Some(OTHER_UUID)).await;
    let e = b.fence("m1").await.expect_err("wrong server");
    assert!(e.contains("refusing") && e.contains(OTHER_UUID), "{e}");
    assert!(b.power_state("m1").await.is_err());
    assert_eq!(fake.lock().expect("fake").offs, 0);
    let (b, fake) = maas(true, None).await;
    let e = b.fence("m1").await.expect_err("no uuid");
    assert!(e.contains("commission"), "{e}");
    assert_eq!(fake.lock().expect("fake").offs, 0);
}

#[tokio::test]
async fn maas_accepting_a_power_off_that_leaves_the_server_on_is_not_evidence() {
    let (b, fake) = maas(true, Some(UUID)).await;
    fake.lock().expect("fake").stays_on = true;
    let e = b.fence("m1").await.expect_err("still on");
    assert!(e.contains("still does not report"), "{e}");
}

#[test]
fn a_maas_entry_needs_its_own_settings_and_a_valid_identity() {
    let ok = |t: &str| Mapping::parse(t, false);
    let entry = |extra: &str| {
        format!(
            r#"{{ "m1": {{ "provider": "maas", "systemId": "abc123", "uuid": "{UUID}" {extra} }} }}"#
        )
    };
    assert!(ok(&entry("")).is_ok());
    assert!(ok(&entry("").replace("abc123", "AB;c")).is_err());
    assert!(
        ok(&entry("").replace(UUID, "nope")).is_err(),
        "the hardware UUID is required"
    );
    assert!(
        ok(&entry(r#", "secretRef": "X""#)).is_err(),
        "no credentials in the mapping"
    );
    let dir = std::env::temp_dir().join(format!("apelora-bmc-maas-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let file = dir.join("map.json");
    std::fs::write(&file, entry("")).expect("write");
    let env = |with: &'static [(&'static str, &'static str)]| {
        let f = file.display().to_string();
        move |k: &str| match k {
            "APELORA_BMC_MAP" => Some(f.clone()),
            other => with
                .iter()
                .find(|(n, _)| *n == other)
                .map(|(_, v)| (*v).to_owned()),
        }
    };
    let e = bmc_from_env(env(&[])).expect_err("needs MAAS settings");
    assert!(e.contains("APELORA_MAAS_API_KEY"), "{e}");
    let e = bmc_from_env(env(&[("APELORA_MAAS_API_KEY", "c:t:s")])).expect_err("needs a url");
    assert!(e.contains("APELORA_MAAS_URL"), "{e}");
    let e = bmc_from_env(env(&[
        ("APELORA_MAAS_API_KEY", "c:t:s"),
        ("APELORA_MAAS_URL", "http://maas.example/MAAS"),
    ]))
    .expect_err("https");
    assert!(e.contains("https"), "{e}");
    assert!(!e.contains("c:t:s"), "{e}");
    let (b, _) = bmc_from_env(env(&[
        ("APELORA_MAAS_API_KEY", "c:t:s"),
        ("APELORA_MAAS_URL", "https://maas.example/MAAS"),
    ]))
    .expect("configured");
    assert_eq!(b.members(), vec!["m1".to_owned()]);
    let _ = std::fs::remove_dir_all(dir);
}
