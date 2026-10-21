// Directory sign-in (LDAP with TLS) through the real router, against a real PostgreSQL.
//
// The tests without `#[ignore]` use local TLS servers made here (certificates from rcgen), so the
// TLS checks and the deadline are exercised for real without a directory. The `real_directory_*`
// tests need the development directory (REAL DIRECTORY TESTED evidence):
//
// ```sh
// bash infra/dev/directory/make-certs.sh
// docker compose -f infra/dev/compose.yaml --profile directory up -d --build directory
// cargo test -p apelora-control-plane real_directory -- --ignored --test-threads 1
// ```
//
// Override the address or CA with APELORA_TEST_DIRECTORY_URL / APELORA_TEST_DIRECTORY_CA.

use std::sync::Arc;
use std::time::Duration;

use apelora_incus_adapter::MockIncus;
use apelora_persistence::directories::DirectoryFields;
use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair, KeyUsagePurpose};
use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;
use uuid::Uuid;

use super::directories::{upsert_directory_user, verify_directory_sign_in, verify_within};
use super::identity_tests::{code, lab};
use super::ldap::{self, Failure};
use super::tests::send;
use super::*;

/// A certificate authority and a server certificate for `names`.
fn certificates(names: &[&str]) -> (String, rustls::ServerConfig) {
    let ca_key = KeyPair::generate().expect("key");
    let mut ca = CertificateParams::new(Vec::<String>::new()).expect("params");
    ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca.distinguished_name
        .push(DnType::CommonName, "directory test CA");
    ca.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    let ca_cert = ca.self_signed(&ca_key).expect("CA");
    let issuer = Issuer::new(ca, ca_key);
    let key = KeyPair::generate().expect("key");
    let leaf = CertificateParams::new(names.iter().map(|n| (*n).to_owned()).collect::<Vec<_>>())
        .expect("params")
        .signed_by(&key, &issuer)
        .expect("leaf");
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("versions")
    .with_no_client_auth()
    .with_single_cert(
        vec![leaf.der().clone()],
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
    )
    .expect("server config");
    (ca_cert.pem(), config)
}

/// A TLS server on loopback that completes handshakes and then says nothing (no LDAP).
async fn tls_server(names: &[&str]) -> (u16, String) {
    let (ca, config) = certificates(names);
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                if let Ok(mut tls) = acceptor.accept(stream).await {
                    let mut buf = [0u8; 512];
                    while matches!(tls.read(&mut buf).await, Ok(n) if n > 0) {}
                }
            });
        }
    });
    (port, ca)
}

/// Accepts connections and never answers: a directory that hangs.
async fn silent_server() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((stream, _)) = listener.accept().await {
            held.push(stream);
        }
    });
    port
}

/// A loopback port with nothing listening.
async fn closed_port() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    listener.local_addr().expect("addr").port()
}

fn fields(url: String, ca_pem: Option<String>) -> DirectoryFields {
    DirectoryFields {
        name: "Test".into(),
        url,
        ca_pem,
        bind_template: "uid={username},ou=people,dc=apelora,dc=test".into(),
        id_attribute: "entryUUID".into(),
        username_attribute: "uid".into(),
        email_attribute: "mail".into(),
        name_attribute: "cn".into(),
        group_name_attribute: "cn".into(),
        enabled: true,
        ..DirectoryFields::default()
    }
}

#[tokio::test]
async fn tls_problems_are_named_precisely() {
    let (port, ca) = tls_server(&["localhost"]).await;
    let ok = fields(format!("ldaps://localhost:{port}"), Some(ca.clone()));
    assert_eq!(ldap::reach(&ok, Duration::from_secs(5)).await, Ok(()));

    let wrong_host = fields(format!("ldaps://127.0.0.1:{port}"), Some(ca));
    assert_eq!(
        ldap::reach(&wrong_host, Duration::from_secs(5)).await,
        Err(Failure::CertificateNameMismatch)
    );
    let system_roots = fields(format!("ldaps://localhost:{port}"), None);
    assert_eq!(
        ldap::reach(&system_roots, Duration::from_secs(5)).await,
        Err(Failure::CertificateUntrusted)
    );
    let (other_ca, _) = certificates(&["localhost"]);
    let other = fields(format!("ldaps://localhost:{port}"), Some(other_ca));
    assert_eq!(
        ldap::reach(&other, Duration::from_secs(5)).await,
        Err(Failure::CertificateUntrusted)
    );
    let closed = fields(format!("ldaps://127.0.0.1:{}", closed_port().await), None);
    let e = ldap::reach(&closed, Duration::from_secs(5))
        .await
        .expect_err("closed");
    assert!(matches!(e, Failure::Unreachable(_)), "{e:?}");
}

#[tokio::test]
async fn a_directory_that_never_answers_runs_into_the_deadline() {
    let port = silent_server().await;
    let f = fields(format!("ldaps://127.0.0.1:{port}"), None);
    let started = std::time::Instant::now();
    assert_eq!(
        ldap::reach(&f, Duration::from_millis(300)).await,
        Err(Failure::Timeout)
    );
    assert_eq!(
        ldap::sign_in(&f, "ada", "secret", Duration::from_millis(300)).await,
        Err(Failure::Timeout)
    );
    assert!(started.elapsed() < Duration::from_secs(3));
}

fn admin_state(pool: &Pool) -> AppState {
    AppState::new(
        Provider::Mock(MockIncus::default()),
        Auth::Tokens(pool.clone()),
    )
}

async fn admin_token(pool: &Pool) -> String {
    apelora_persistence::create_token(pool, "admin", "fabric-admin", None)
        .await
        .expect("token")
        .0
}

fn body(name: &str, url: &str, ca: Option<&str>) -> Value {
    json!({ "name": name, "url": url, "caPem": ca, "bindTemplate": "uid={username},ou=people,dc=apelora,dc=test" })
}

async fn audit_text(pool: &Pool) -> String {
    let rows: Vec<(String, String, String, Value)> =
        sqlx::query_as("SELECT action, target, outcome, detail FROM audit_events ORDER BY id")
            .fetch_all(pool)
            .await
            .expect("audit");
    serde_json::to_string(&rows).expect("json")
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn directories_are_configured_checked_offered_and_audited(pool: Pool) {
    let state = admin_state(&pool);
    let admin = admin_token(&pool).await;
    let req = |method: &'static str, path: String, body: Option<Value>| {
        let (state, admin) = (state.clone(), admin.clone());
        async move { send(state, method, &path, &admin, body, None).await }
    };
    let (status, s) = req("GET", "/api/v1/identity/sign-in".into(), None).await;
    assert_eq!(status, StatusCode::OK, "{s}");
    assert_eq!(s["directories"], json!([]));

    // Plain LDAP is refused when saving.
    let (status, e) = req(
        "POST",
        "/api/v1/identity/sign-in/directories".into(),
        Some(body("Plain", "ldap://localhost:1389", None)),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        e["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("plain LDAP is refused"),
        "{e}"
    );

    let (port, ca) = tls_server(&["localhost"]).await;
    let (status, lab) = req(
        "POST",
        "/api/v1/identity/sign-in/directories".into(),
        Some(body(
            "Lab",
            &format!("ldaps://LOCALHOST:{port}/"),
            Some(&ca),
        )),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{lab}");
    assert_eq!(lab["url"], format!("ldaps://localhost:{port}"));
    assert_eq!(
        (lab["reachable"].as_bool(), lab["problem"].is_null()),
        (Some(true), true),
        "{lab}"
    );
    let gone = closed_port().await;
    let (status, down) = req(
        "POST",
        "/api/v1/identity/sign-in/directories".into(),
        Some(body("Down", &format!("ldaps://127.0.0.1:{gone}"), None)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{down}");
    assert_eq!(
        (down["reachable"].as_bool(), down["problem"].as_str()),
        (Some(false), Some("unreachable"))
    );
    let (status, e) = req(
        "POST",
        "/api/v1/identity/sign-in/directories".into(),
        Some(body("Lab", "ldaps://other.example.test", None)),
    )
    .await;
    assert_eq!(
        (status, code(&e)),
        (StatusCode::CONFLICT, Some("directory_exists"))
    );

    let (_, s) = req("GET", "/api/v1/identity/sign-in".into(), None).await;
    let names: Vec<(&str, bool)> = s["directories"]
        .as_array()
        .expect("list")
        .iter()
        .map(|d| {
            (
                d["name"].as_str().unwrap_or_default(),
                d["reachable"].as_bool().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(names, vec![("Down", false), ("Lab", true)]);
    let (status, l) = req("GET", "/api/v1/identity/sign-in/directories".into(), None).await;
    assert_eq!(
        (status, l["items"].as_array().map(Vec::len)),
        (StatusCode::OK, Some(2))
    );

    // The sign-in page offers the enabled ones, by id and name only.
    let (_, opts) = send(state.clone(), "GET", "/api/v1/sign-in", "", None, None).await;
    assert_eq!(
        opts["directories"].as_array().map(Vec::len),
        Some(2),
        "{opts}"
    );
    assert_eq!(
        opts["directories"][0],
        json!({ "id": down["id"], "name": "Down" })
    );
    let down_id = down["id"].as_str().expect("id").to_owned();
    let mut off = body("Down", &format!("ldaps://127.0.0.1:{gone}"), None);
    off["enabled"] = json!(false);
    let (status, d) = req(
        "PUT",
        format!("/api/v1/identity/sign-in/directories/{down_id}"),
        Some(off),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{d}");
    assert_eq!(
        (
            d["enabled"].as_bool(),
            d["reachable"].as_bool(),
            d["problem"].is_null()
        ),
        (Some(false), Some(false), true)
    );
    let (_, opts) = send(state.clone(), "GET", "/api/v1/sign-in", "", None, None).await;
    assert_eq!(
        opts["directories"],
        json!([{ "id": lab["id"], "name": "Lab" }])
    );
    let (status, _) = req(
        "PUT",
        format!("/api/v1/identity/sign-in/directories/{}", Uuid::new_v4()),
        Some(body("Nope", "ldaps://x.example.test", None)),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Check: never without a test password; precise reasons.
    let check = |settings: Value, username: &str, password: &str| json!({ "settings": settings, "username": username, "password": password });
    let (status, e) = req(
        "POST",
        "/api/v1/identity/sign-in/directories/check".into(),
        Some(check(
            body("Down", &format!("ldaps://127.0.0.1:{gone}"), None),
            "ada",
            "",
        )),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{e}");
    let (status, c) = req(
        "POST",
        "/api/v1/identity/sign-in/directories/check".into(),
        Some(check(
            body("Down", &format!("ldaps://127.0.0.1:{gone}"), None),
            "ada",
            "check-secret-1",
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{c}");
    assert_eq!(
        (c["ok"].as_bool(), c["problem"].as_str()),
        (Some(false), Some("unreachable")),
        "{c}"
    );
    let (_, c) = req(
        "POST",
        "/api/v1/identity/sign-in/directories/check".into(),
        Some(check(
            body("Lab", &format!("ldaps://127.0.0.1:{port}"), Some(&ca)),
            "ada",
            "check-secret-1",
        )),
    )
    .await;
    assert_eq!(c["problem"], "certificate-name-mismatch", "{c}");

    // Sign-in: a turned-off directory is not offered; an empty password never reaches it; a
    // directory that hangs answers 503 within the deadline.
    let lab_id: Uuid = lab["id"].as_str().expect("id").parse().expect("uuid");
    let e = verify_directory_sign_in(&state, down_id.parse().expect("uuid"), "ada", "x")
        .await
        .expect_err("off");
    assert_eq!(e.code, "directory_unavailable");
    let e = verify_directory_sign_in(&state, lab_id, "ada", "")
        .await
        .expect_err("empty");
    assert_eq!(
        (e.status, e.code),
        (StatusCode::UNAUTHORIZED, "invalid_credentials")
    );
    assert_eq!(e.message, "The username or password is incorrect.");
    let silent = silent_server().await;
    let (status, hang) = req(
        "PUT",
        format!("/api/v1/identity/sign-in/directories/{lab_id}"),
        Some(body(
            "Lab",
            &format!("ldaps://localhost:{silent}"),
            Some(&ca),
        )),
    )
    .await;
    assert_eq!(
        (status, hang["problem"].as_str()),
        (StatusCode::OK, Some("timeout")),
        "{hang}"
    );
    let e = verify_within(
        &state,
        lab_id,
        "ada",
        "sign-in-secret-2",
        Duration::from_millis(300),
    )
    .await
    .expect_err("hangs");
    assert_eq!(
        (e.status, e.code),
        (StatusCode::SERVICE_UNAVAILABLE, "directory_unreachable")
    );
    assert!(e.message.contains("Lab"), "{}", e.message);

    // Removing needs the name typed.
    let path = format!("/api/v1/identity/sign-in/directories/{down_id}");
    let (status, e) = req("DELETE", path.clone(), Some(json!({ "confirm": "down" }))).await;
    assert_eq!(
        (status, code(&e)),
        (StatusCode::BAD_REQUEST, Some("confirmation_mismatch"))
    );
    let (status, _) = req("DELETE", path.clone(), Some(json!({ "confirm": "Down" }))).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = req("DELETE", path, Some(json!({ "confirm": "Down" }))).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let audit = audit_text(&pool).await;
    let count = |action: &str| audit.matches(&format!("\"{action}\"")).count();
    assert_eq!(
        (
            count("identity.directory.saved"),
            count("identity.directory.deleted"),
            count("auth.directory.failed")
        ),
        (4, 1, 2),
        "{audit}"
    );
    assert!(
        audit.contains("\"reason\":\"unreachable\"") && audit.contains("\"username\":\"ada\""),
        "{audit}"
    );
    for secret in ["check-secret-1", "sign-in-secret-2"] {
        assert!(
            !audit.contains(secret),
            "a password reached the audit log: {audit}"
        );
    }
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn directory_settings_need_platform_security_manage(pool: Pool) {
    let l = lab(pool.clone());
    l.user("sub-nobody", "nobody").await;
    let token = l.token("sub-nobody", "nobody");
    let id = Uuid::new_v4();
    let settings = body("X", "ldaps://ldap.example.test", None);
    for (method, path, body) in [
        (
            "GET",
            "/api/v1/identity/sign-in/directories".to_owned(),
            None,
        ),
        (
            "POST",
            "/api/v1/identity/sign-in/directories".to_owned(),
            Some(settings.clone()),
        ),
        (
            "POST",
            "/api/v1/identity/sign-in/directories/check".to_owned(),
            Some(json!({ "settings": settings, "username": "a", "password": "b" })),
        ),
        (
            "PUT",
            format!("/api/v1/identity/sign-in/directories/{id}"),
            Some(settings.clone()),
        ),
        (
            "DELETE",
            format!("/api/v1/identity/sign-in/directories/{id}"),
            Some(json!({ "confirm": "X" })),
        ),
    ] {
        let (status, e) = l.req(method, &path, &token, body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path}: {e}");
    }
    let (status, _) = send(
        l.state.clone(),
        "GET",
        "/api/v1/identity/sign-in/directories",
        "",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// ---- REAL DIRECTORY TESTED (ignored by default; see the top of this file) ----------------------

const DEV_PASSWORD: &str = "apelora-dev-only";

fn real_url() -> String {
    std::env::var("APELORA_TEST_DIRECTORY_URL").unwrap_or_else(|_| "ldaps://localhost:1636".into())
}

fn real_ca() -> String {
    let path = std::env::var("APELORA_TEST_DIRECTORY_CA").unwrap_or_else(|_| {
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../infra/dev/directory/certs/ca.pem"
        )
        .into()
    });
    std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!("{path}: {e}; run infra/dev/directory/make-certs.sh and start the directory")
    })
}

/// Saves the development directory through the API and returns its id.
async fn save_real(state: &AppState, admin: &str, extra: Value) -> Uuid {
    let mut b = json!({ "name": "Development directory", "url": real_url(), "caPem": real_ca(),
        "bindTemplate": "uid={username},ou=people,dc=apelora,dc=test", "groupAttribute": "memberOf" });
    if let (Some(o), Some(x)) = (b.as_object_mut(), extra.as_object()) {
        o.extend(x.clone());
    }
    let (status, d) = send(
        state.clone(),
        "POST",
        "/api/v1/identity/sign-in/directories",
        admin,
        Some(b),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{d}");
    assert_eq!(
        d["reachable"], true,
        "the development directory answers with TLS: {d}"
    );
    d["id"].as_str().expect("id").parse().expect("uuid")
}

#[ignore = "needs the development directory: docker compose -f infra/dev/compose.yaml --profile directory up -d"]
#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn real_directory_correct_password_gives_the_person_with_groups(pool: Pool) {
    let state = admin_state(&pool);
    let admin = admin_token(&pool).await;
    let id = save_real(&state, &admin, json!({})).await;

    let who = verify_directory_sign_in(&state, id, "ada", DEV_PASSWORD)
        .await
        .unwrap_or_else(|e| panic!("{}", e.message));
    assert_eq!(who.issuer, real_url());
    assert_eq!(
        (
            who.username.as_str(),
            who.email.as_deref(),
            who.display_name.as_deref()
        ),
        ("ada", Some("ada@apelora.test"), Some("Ada Admin"))
    );
    assert_eq!(who.groups, vec!["dev-admins", "dev-operators"]);
    assert_eq!(who.subject.len(), 36, "entryUUID: {}", who.subject);
    let user = upsert_directory_user(&state, &who)
        .await
        .unwrap_or_else(|e| panic!("{}", e.message));
    assert_eq!(
        (user.issuer.as_str(), user.subject.as_str()),
        (real_url().as_str(), who.subject.as_str())
    );
    // The same person again: the same row.
    let again = verify_directory_sign_in(&state, id, "ada", DEV_PASSWORD)
        .await
        .unwrap_or_else(|e| panic!("{}", e.message));
    assert_eq!(
        upsert_directory_user(&state, &again)
            .await
            .map(|u| u.id)
            .ok(),
        Some(user.id)
    );

    // A "+" in the uid: the DN escaping matches what the directory expects.
    let sam = verify_directory_sign_in(&state, id, "sam+ops", DEV_PASSWORD)
        .await
        .unwrap_or_else(|e| panic!("{}", e.message));
    assert_eq!(
        (sam.username.as_str(), sam.groups.clone()),
        ("sam+ops", vec!["dev-operators".to_owned()])
    );
    let vic = verify_directory_sign_in(&state, id, "vic", DEV_PASSWORD)
        .await
        .unwrap_or_else(|e| panic!("{}", e.message));
    assert!(vic.groups.is_empty());

    // Groups by search instead of memberOf, and the check shows the Apelora groups they map to.
    apelora_persistence::identity::create_group(
        &pool,
        None,
        "Platform operators",
        Some("dev-operators"),
        "",
    )
    .await
    .expect("group");
    let mut search = json!({ "name": "By search", "url": real_url(), "caPem": real_ca(),
        "bindTemplate": "uid={username},ou=people,dc=apelora,dc=test", "groupBaseDn": "ou=groups,dc=apelora,dc=test" });
    let (status, c) = send(
        state.clone(),
        "POST",
        "/api/v1/identity/sign-in/directories/check",
        &admin,
        Some(json!({ "settings": search.clone(), "username": "oscar", "password": DEV_PASSWORD })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{c}");
    assert_eq!(c["ok"], true, "{c}");
    assert_eq!(c["entry"]["groups"], json!(["dev-operators"]));
    assert_eq!(c["entry"]["mappedGroups"], json!(["Platform operators"]));
    assert_eq!(c["entry"]["dn"], "uid=oscar,ou=people,dc=apelora,dc=test");
    // A user filter under a base DN finds the same person.
    search["baseDn"] = json!("ou=people,dc=apelora,dc=test");
    search["userFilter"] = json!("(&(objectClass=inetOrgPerson)(uid={username}))");
    let (_, c) = send(
        state.clone(),
        "POST",
        "/api/v1/identity/sign-in/directories/check",
        &admin,
        Some(json!({ "settings": search, "username": "oscar", "password": DEV_PASSWORD })),
        None,
    )
    .await;
    assert_eq!(
        (c["ok"].as_bool(), c["entry"]["username"].as_str()),
        (Some(true), Some("oscar")),
        "{c}"
    );

    let audit = audit_text(&pool).await;
    assert_eq!(
        audit.matches("\"auth.directory.succeeded\"").count(),
        2,
        "{audit}"
    );
    assert!(
        !audit.contains(DEV_PASSWORD),
        "a password reached the audit log"
    );
}

#[ignore = "needs the development directory: docker compose -f infra/dev/compose.yaml --profile directory up -d"]
#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn real_directory_wrong_password_and_injection_attempts_are_invalid_credentials(pool: Pool) {
    let state = admin_state(&pool);
    let admin = admin_token(&pool).await;
    let id = save_real(&state, &admin, json!({})).await;
    for (username, password) in [
        ("ada", "wrong-password-3"),
        ("nobody-here", "wrong-password-3"),
        // DN injection: the comma and equals stay inside the uid value.
        ("ada,ou=people", DEV_PASSWORD),
        ("*", DEV_PASSWORD),
    ] {
        let e = verify_directory_sign_in(&state, id, username, password)
            .await
            .expect_err(username);
        assert_eq!(
            (e.status, e.code),
            (StatusCode::UNAUTHORIZED, "invalid_credentials"),
            "{username}: {}",
            e.message
        );
    }
    // Filter injection: with a user filter, the wildcard and parentheses are escaped, so the
    // search finds nobody else (and the bind as "*)(uid=*" fails first anyway).
    let (status, d) = send(state.clone(), "PUT", &format!("/api/v1/identity/sign-in/directories/{id}"), &admin,
        Some(json!({ "name": "Development directory", "url": real_url(), "caPem": real_ca(), "bindTemplate": "uid={username},ou=people,dc=apelora,dc=test",
            "baseDn": "ou=people,dc=apelora,dc=test", "userFilter": "(uid={username})" })), None).await;
    assert_eq!(status, StatusCode::OK, "{d}");
    let e = verify_directory_sign_in(&state, id, "*)(uid=*", DEV_PASSWORD)
        .await
        .expect_err("filter injection");
    assert_eq!(e.code, "invalid_credentials");
    let ok = verify_directory_sign_in(&state, id, "ada", DEV_PASSWORD)
        .await
        .unwrap_or_else(|e| panic!("{}", e.message));
    assert_eq!(ok.username, "ada");
    let audit = audit_text(&pool).await;
    assert_eq!(
        audit.matches("\"auth.directory.failed\"").count(),
        5,
        "{audit}"
    );
    assert!(audit.contains("\"reason\":\"wrong\""), "{audit}");
    assert!(
        !audit.contains("wrong-password-3") && !audit.contains(DEV_PASSWORD),
        "a password reached the audit log"
    );
}

#[ignore = "needs the development directory: docker compose -f infra/dev/compose.yaml --profile directory up -d"]
#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn real_directory_tls_is_required_and_checked(pool: Pool) {
    let state = admin_state(&pool);
    let admin = admin_token(&pool).await;
    let check = |settings: Value| {
        let (state, admin) = (state.clone(), admin.clone());
        async move {
            send(
                state,
                "POST",
                "/api/v1/identity/sign-in/directories/check",
                &admin,
                Some(json!({ "settings": settings, "username": "ada", "password": DEV_PASSWORD })),
                None,
            )
            .await
        }
    };
    let starttls_url = real_url()
        .replace("ldaps://", "ldap://")
        .replace(":1636", ":1389");
    // Plain LDAP is refused before anything is sent; StartTLS on the same port works.
    let (status, e) = check(json!({ "name": "Plain", "url": starttls_url, "bindTemplate": "uid={username},ou=people,dc=apelora,dc=test" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{e}");
    let (_, c) = check(json!({ "name": "StartTLS", "url": starttls_url, "startTls": true, "caPem": real_ca(), "bindTemplate": "uid={username},ou=people,dc=apelora,dc=test" })).await;
    assert_eq!(c["ok"], true, "{c}");
    // The development CA is not among the system's roots.
    let (_, c) = check(json!({ "name": "No CA", "url": real_url(), "bindTemplate": "uid={username},ou=people,dc=apelora,dc=test" })).await;
    assert_eq!(c["problem"], "certificate-untrusted", "{c}");
    let (_, c) = check(json!({ "name": "StartTLS no CA", "url": starttls_url, "startTls": true, "bindTemplate": "uid={username},ou=people,dc=apelora,dc=test" })).await;
    assert_eq!(c["problem"], "certificate-untrusted", "{c}");
    // The certificate names localhost, not 127.0.0.1.
    let (_, c) = check(json!({ "name": "By address", "url": real_url().replace("localhost", "127.0.0.1"), "caPem": real_ca(), "bindTemplate": "uid={username},ou=people,dc=apelora,dc=test" })).await;
    assert_eq!(c["problem"], "certificate-name-mismatch", "{c}");
    // A wrong password in the check says so, and an entry the bind cannot find is named.
    let (_, c) = send(state.clone(), "POST", "/api/v1/identity/sign-in/directories/check", &admin,
        Some(json!({ "settings": { "name": "Lab", "url": real_url(), "caPem": real_ca(), "bindTemplate": "uid={username},ou=people,dc=apelora,dc=test" }, "username": "ada", "password": "nope" })), None).await;
    assert_eq!(c["problem"], "bind-refused", "{c}");
    let (_, c) = send(state.clone(), "POST", "/api/v1/identity/sign-in/directories/check", &admin,
        Some(json!({ "settings": { "name": "Lab", "url": real_url(), "caPem": real_ca(), "bindTemplate": "uid={username},ou=people,dc=apelora,dc=test",
            "baseDn": "ou=groups,dc=apelora,dc=test", "userFilter": "(uid={username})" }, "username": "ada", "password": DEV_PASSWORD })), None).await;
    assert_eq!(c["problem"], "entry-not-found", "{c}");
}
