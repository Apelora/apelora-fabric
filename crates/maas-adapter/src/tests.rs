// CONTRACT VERIFIED: a MAAS-shaped server on loopback (the documented fields and paths, not a
// real MAAS), and the pure parts: the key, the signature, the percent-encoding, the parsing.

use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde_json::json;

use super::*;

const KEY: &str = "consumerkey:tokenkey:s3cret/value+here";

#[derive(Default)]
struct Fake {
    power: String,
    /// Requests seen: "METHOD path body".
    seen: Vec<String>,
    stays_on: bool,
    reject_auth: bool,
}
type Shared = Arc<Mutex<Fake>>;

fn machine(id: &str, host: &str, power: &str) -> Value {
    json!({ "system_id": id, "hostname": host, "fqdn": format!("{host}.maas"), "status": 6, "status_name": "Deployed",
        "power_state": power, "power_type": "redfish", "architecture": "arm64/generic", "cpu_count": 8, "memory": 16384,
        "osystem": "ubuntu", "distro_series": "noble", "ip_addresses": ["10.0.0.11", "fd00::11"],
        "hardware_uuid": "4c4c4544-0042-4c10-8051-b7c04f435331", "zone": { "name": "rack-a" },
        "pool": { "name": "default" }, "owner": "apelora", "power_parameters": { "power_pass": "never shown" } })
}

fn authed(s: &Shared, h: &HeaderMap, line: String) -> Option<Response> {
    let mut f = s.lock().expect("fake");
    f.seen.push(line);
    let auth = h
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let ok = auth.contains("oauth_signature_method=\"PLAINTEXT\"")
        && auth.contains("oauth_consumer_key=\"consumerkey\"")
        && auth.contains("oauth_token=\"tokenkey\"")
        && auth.contains("oauth_signature=\"%26s3cret%2Fvalue%2Bhere\"")
        && auth.contains("oauth_nonce=\"")
        && auth.contains("oauth_timestamp=\"");
    (f.reject_auth || !ok).then(|| StatusCode::UNAUTHORIZED.into_response())
}

fn json_response(v: Value) -> Response {
    ([("content-type", "application/json")], v.to_string()).into_response()
}

async fn list(State(s): State<Shared>, h: HeaderMap) -> Response {
    if let Some(r) = authed(&s, &h, "GET machines/".into()) {
        return r;
    }
    let mut new = machine("n3wmch", "rack-a-new", "unknown");
    new["status_name"] = json!("New");
    new["hardware_uuid"] = Value::Null;
    new["owner"] = Value::Null;
    new["ip_addresses"] = json!([]);
    let power = s.lock().expect("fake").power.clone();
    json_response(json!([machine("abc123", "rack-a-1", &power), new]))
}

async fn one(State(s): State<Shared>, h: HeaderMap, Path(id): Path<String>) -> Response {
    if let Some(r) = authed(&s, &h, format!("GET machines/{id}/")) {
        return r;
    }
    let power = s.lock().expect("fake").power.clone();
    json_response(machine(&id, "rack-a-1", &power))
}

async fn query(State(s): State<Shared>, h: HeaderMap, Path(id): Path<String>) -> Response {
    if let Some(r) = authed(&s, &h, format!("POST machines/{id}/op-query_power_state")) {
        return r;
    }
    let power = s.lock().expect("fake").power.clone();
    json_response(json!({ "state": power }))
}

async fn off(
    State(s): State<Shared>,
    h: HeaderMap,
    Path(id): Path<String>,
    body: String,
) -> Response {
    if let Some(r) = authed(&s, &h, format!("POST machines/{id}/op-power_off {body}")) {
        return r;
    }
    let mut f = s.lock().expect("fake");
    if !f.stays_on {
        f.power = "off".into();
    }
    json_response(machine(&id, "rack-a-1", &f.power))
}

async fn accept(State(s): State<Shared>, h: HeaderMap, req: axum::extract::Request) -> Response {
    let path = req.uri().path().to_owned();
    let body = axum::body::to_bytes(req.into_body(), 256 * 1024)
        .await
        .unwrap_or_default();
    let line = format!("POST {path} {}", String::from_utf8_lossy(&body));
    if let Some(r) = authed(&s, &h, line) {
        return r;
    }
    json_response(machine("abc123", "rack-a-1", "on"))
}

async fn server(power: &str) -> (MaasClient, Shared) {
    let shared: Shared = Arc::new(Mutex::new(Fake {
        power: power.into(),
        ..Default::default()
    }));
    let app = Router::new()
        .route("/MAAS/api/2.0/machines/", get(list))
        .route("/MAAS/api/2.0/machines/{id}/", get(one))
        .route(
            "/MAAS/api/2.0/machines/{id}/op-query_power_state",
            post(query),
        )
        .route("/MAAS/api/2.0/machines/{id}/op-power_off", post(off))
        .route("/MAAS/api/2.0/machines/{id}/op-commission", post(accept))
        .route("/MAAS/api/2.0/machines/{id}/op-deploy", post(accept))
        .route("/MAAS/api/2.0/machines/{id}/op-release", post(accept))
        .with_state(shared.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move { axum::serve(listener, app).await.ok() });
    (
        MaasClient::plain_for_tests(
            &format!("http://{addr}/MAAS"),
            ApiKey::parse(KEY).expect("key"),
        ),
        shared,
    )
}

#[test]
fn the_key_is_three_parts_prints_redacted_and_signs_with_plaintext() {
    let k = ApiKey::parse(KEY).expect("key");
    assert_eq!(format!("{k:?}"), "ApiKey([redacted])");
    for bad in ["", "a:b", "a::c", ":b:c", "a:b:c d", "a:b:é"] {
        assert!(ApiKey::parse(bad).is_err(), "{bad:?}");
    }
    // The secret may itself contain colons.
    assert!(ApiKey::parse("a:b:c:d").is_ok());
    assert_eq!(
        k.header("nonce1", 1_700_000_000),
        "OAuth realm=\"\", oauth_version=\"1.0\", oauth_signature_method=\"PLAINTEXT\", oauth_consumer_key=\"consumerkey\", oauth_token=\"tokenkey\", oauth_signature=\"%26s3cret%2Fvalue%2Bhere\", oauth_nonce=\"nonce1\", oauth_timestamp=\"1700000000\""
    );
    assert_eq!(encode("a b&c=d~e-f.g_h"), "a%20b%26c%3Dd~e-f.g_h");
}

#[test]
fn the_client_needs_https_and_no_credentials_in_the_address() {
    let key = || ApiKey::parse(KEY).expect("key");
    assert!(MaasClient::new("https://maas.example:5240/MAAS", key(), None).is_ok());
    for bad in [
        "http://maas.example/MAAS",
        "https://u:p@maas.example/MAAS",
        "https://maas.example/MAAS?x=1",
        "maas.example",
        "",
    ] {
        assert!(MaasClient::new(bad, key(), None).is_err(), "{bad:?}");
    }
    assert!(
        MaasClient::new(
            "https://maas.example/MAAS",
            key(),
            Some(std::path::Path::new("/nonexistent/ca.pem"))
        )
        .is_err()
    );
    let c = MaasClient::new("https://maas.example/MAAS/", key(), None).expect("client");
    assert!(!format!("{c:?}").contains("s3cret"), "{c:?}");
}

#[tokio::test]
async fn machines_are_listed_with_what_apelora_uses_and_nothing_from_their_power_parameters() {
    let (c, _) = server("on").await;
    let all = c.machines().await.expect("machines");
    assert_eq!(all.len(), 2);
    let m = &all[0];
    assert_eq!(
        (
            m.system_id.as_str(),
            m.hostname.as_str(),
            m.status.as_str(),
            m.power_state,
            m.power_type.as_str()
        ),
        ("abc123", "rack-a-1", "Deployed", PowerState::On, "redfish")
    );
    assert_eq!(
        (
            m.cpu_count,
            m.memory_mib,
            m.os.as_str(),
            m.zone.as_str(),
            m.pool.as_str()
        ),
        (8, 16384, "ubuntu noble", "rack-a", "default")
    );
    assert_eq!(
        m.addresses,
        vec!["10.0.0.11".to_owned(), "fd00::11".to_owned()]
    );
    assert_eq!(
        m.hardware_uuid.as_deref(),
        Some("4c4c4544-0042-4c10-8051-b7c04f435331")
    );
    assert!(
        !format!("{all:?}").contains("never shown"),
        "credential fields are not carried"
    );
    // A machine MAAS has not commissioned has no hardware UUID and an unknown power state.
    let n = &all[1];
    assert_eq!(
        (
            n.status.as_str(),
            n.power_state,
            n.hardware_uuid.as_deref(),
            n.owner.as_deref()
        ),
        ("New", PowerState::Unknown, None, None)
    );
    assert_eq!(c.machine("abc123").await.expect("one").hostname, "rack-a-1");
}

#[tokio::test]
async fn a_machine_is_switched_off_hard_and_the_state_is_read_back_separately() {
    let (c, fake) = server("on").await;
    assert_eq!(c.query_power_state("abc123").await, Ok(PowerState::On));
    c.power_off("abc123").await.expect("accepted");
    assert_eq!(c.query_power_state("abc123").await, Ok(PowerState::Off));
    let seen = fake.lock().expect("fake").seen.clone();
    assert!(
        seen.iter()
            .any(|l| l == "POST machines/abc123/op-power_off stop_mode=hard"),
        "{seen:?}"
    );
    // Accepted but not off: the caller sees it on the read-back, not in the answer.
    let (c, fake) = server("on").await;
    fake.lock().expect("fake").stays_on = true;
    c.power_off("abc123").await.expect("accepted");
    assert_eq!(c.query_power_state("abc123").await, Ok(PowerState::On));
}

#[tokio::test]
async fn a_system_id_that_is_not_one_never_reaches_the_network() {
    let (c, fake) = server("on").await;
    for bad in ["", "AB", "abc/../x", "abc123;rm", "a b c", &"a".repeat(40)] {
        assert!(
            matches!(c.machine(bad).await, Err(MaasError::Invalid(_))),
            "{bad:?}"
        );
        assert!(
            matches!(c.power_off(bad).await, Err(MaasError::Invalid(_))),
            "{bad:?}"
        );
    }
    assert!(fake.lock().expect("fake").seen.is_empty());
}

#[tokio::test]
async fn refusals_unreachability_and_nonsense_are_told_apart_and_never_carry_the_key() {
    let (c, fake) = server("on").await;
    fake.lock().expect("fake").reject_auth = true;
    let e = c.machines().await.expect_err("401");
    assert_eq!(
        e,
        MaasError::Refused {
            status: 401,
            what: "machines".into()
        }
    );
    assert!(!e.to_string().contains("s3cret"), "{e}");
    // Nothing listens: unreachable.
    let dead =
        MaasClient::plain_for_tests("http://127.0.0.1:9/MAAS", ApiKey::parse(KEY).expect("key"));
    let e = dead.machines().await.expect_err("down");
    assert!(matches!(e, MaasError::Unreachable(_)), "{e:?}");
    assert!(!e.to_string().contains("s3cret"), "{e}");
    // A list that is not a list.
    assert!(matches!(
        Machine::from_json(&json!({ "hostname": "x" })),
        Err(MaasError::Decode(_))
    ));
}

#[tokio::test]
async fn commissioning_and_deploy_are_posted_and_the_cloud_init_is_encoded() {
    let (c, fake) = server("on").await;
    c.commission("abc123").await.expect("commission");
    c.deploy("abc123", "noble", "#cloud-config\nhostname: lab\n")
        .await
        .expect("deploy");
    let seen = fake.lock().expect("fake").seen.clone();
    assert!(seen.iter().any(|l| l.contains("op-commission")), "{seen:?}");
    let deploy = seen
        .iter()
        .find(|l| l.contains("op-deploy"))
        .expect("deploy");
    assert!(deploy.contains("distro_series=noble"), "{deploy}");
    assert!(
        !deploy.contains("#cloud-config"),
        "cloud-init is base64, not the raw text: {deploy}"
    );
    c.release("abc123", false).await.expect("release");
    let seen = fake.lock().expect("fake").seen.clone();
    let release = seen
        .iter()
        .find(|l| l.contains("op-release"))
        .expect("release");
    assert!(release.contains("erase=false"), "{release}");
    c.release("abc123", true).await.expect("erase");
    let seen = fake.lock().expect("fake").seen.clone();
    assert!(
        seen.iter()
            .any(|l| l.contains("op-release") && l.contains("erase=true"))
    );
    assert!(c.deploy("abc123", "noble", "   ").await.is_err());
    assert!(c.deploy("no", "noble", "x").await.is_err());
}
