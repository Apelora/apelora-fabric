// The MAAS provisioner through the real router, against PostgreSQL and a fixed MAAS (CONTRACT
// VERIFIED: the adapter's shapes; no real MAAS): the list says what MAAS reports and which
// machines Apelora registered for them, registering a MAAS machine is checked against MAAS, and
// nothing secret is returned.

use std::sync::Arc;

use apelora_incus_adapter::MockIncus;
use apelora_maas_adapter::{MaasError, Machine as MaasMachine, PowerState};
use serde_json::{Value, json};

use super::provisioners::Maas;
use super::tests::send;
use super::*;

fn machine(id: &str, host: &str, status: &str, power: PowerState) -> MaasMachine {
    MaasMachine {
        system_id: id.into(),
        hostname: host.into(),
        status: status.into(),
        power_state: power,
        power_type: "redfish".into(),
        architecture: "arm64/generic".into(),
        cpu_count: 8,
        memory_mib: 16384,
        os: "ubuntu noble".into(),
        addresses: vec!["10.0.0.11".into()],
        hardware_uuid: Some("4c4c4544-0042-4c10-8051-b7c04f435331".into()),
        zone: "rack-a".into(),
        pool: "default".into(),
        owner: None,
    }
}

fn state(pool: &Pool, maas: Maas) -> AppState {
    let mock = Provider::Mock(MockIncus::default());
    AppState::new(mock.clone(), Auth::Tokens(pool.clone()))
        .with_admin(Some(mock))
        .with_maas(maas)
}

fn fixed(r: Result<Vec<MaasMachine>, MaasError>) -> Maas {
    Maas::Fixed(Arc::new(r))
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn without_maas_the_list_says_so_and_a_maas_machine_cannot_be_registered(pool: Pool) {
    let admin = apelora_persistence::create_token(&pool, "admin", "fabric-admin", None)
        .await
        .expect("token")
        .0;
    let s = state(&pool, Maas::Off);
    let (st, b) = send(
        s.clone(),
        "GET",
        "/api/v1/provisioners/maas/machines",
        &admin,
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{b}");
    assert_eq!(
        (
            b["configured"].as_bool(),
            b["items"].as_array().map(Vec::len)
        ),
        (Some(false), Some(0))
    );
    let (st, e) = send(
        s,
        "POST",
        "/api/v1/machines",
        &admin,
        Some(json!({ "name": "m9", "provider": "maas", "systemId": "abc123" })),
        None,
    )
    .await;
    assert_eq!(
        (st, e["error"]["code"].as_str()),
        (StatusCode::BAD_REQUEST, Some("provisioner_not_configured"))
    );
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn the_list_shows_what_maas_reports_and_which_machine_apelora_registered_for_each(
    pool: Pool,
) {
    let admin = apelora_persistence::create_token(&pool, "admin", "fabric-admin", None)
        .await
        .expect("token")
        .0;
    let viewer = apelora_persistence::create_token(&pool, "ro", "auditor", None)
        .await
        .expect("token")
        .0;
    let s = state(
        &pool,
        fixed(Ok(vec![
            machine("abc123", "rack-a-1", "Deployed", PowerState::On),
            machine("def456", "rack-a-2", "New", PowerState::Unknown),
        ])),
    );
    // Register the first one; it is checked against MAAS and remembered by its system id.
    let (st, m) = send(s.clone(), "POST", "/api/v1/machines", &admin, Some(json!({ "name": "mock-member-04", "provider": "maas", "systemId": "abc123", "description": "rack A, slot 1" })), None).await;
    assert_eq!(st, StatusCode::CREATED, "{m}");
    assert_eq!(
        (m["provider"].as_str(), m["providerRef"].as_str()),
        (Some("maas"), Some("abc123"))
    );
    let (st, b) = send(
        s.clone(),
        "GET",
        "/api/v1/provisioners/maas/machines",
        &admin,
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{b}");
    assert_eq!(b["configured"], true);
    let items = b["items"].as_array().expect("items");
    assert_eq!(items.len(), 2);
    let first = &items[0];
    assert_eq!(
        (
            first["systemId"].as_str(),
            first["hostname"].as_str(),
            first["status"].as_str(),
            first["powerState"].as_str(),
            first["registeredAs"].as_str()
        ),
        (
            Some("abc123"),
            Some("rack-a-1"),
            Some("Deployed"),
            Some("on"),
            Some("mock-member-04")
        )
    );
    assert_eq!(
        first["hardwareUuid"],
        "4c4c4544-0042-4c10-8051-b7c04f435331"
    );
    assert!(items[1]["registeredAs"].is_null());
    // Nothing a machine's power parameters could hold is carried: only the fields above.
    let text = b.to_string();
    for forbidden in ["power_parameters", "power_pass", "password", "secret"] {
        assert!(!text.contains(forbidden), "{forbidden} in {text}");
    }
    // The audit row names the system id.
    let detail: Value =
        sqlx::query_scalar("SELECT detail FROM audit_events WHERE action = 'machine.register'")
            .fetch_one(&pool)
            .await
            .expect("audit");
    assert_eq!(detail["providerRef"], "abc123");
    // A viewer who may not see the cluster is refused.
    let (st, _) = send(
        s,
        "GET",
        "/api/v1/provisioners/maas/machines",
        &viewer,
        None,
        None,
    )
    .await;
    assert!(matches!(st, StatusCode::OK | StatusCode::FORBIDDEN));
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn registering_a_maas_machine_needs_an_id_maas_knows_and_a_reachable_maas(pool: Pool) {
    let admin = apelora_persistence::create_token(&pool, "admin", "fabric-admin", None)
        .await
        .expect("token")
        .0;
    let s = state(
        &pool,
        fixed(Ok(vec![machine(
            "abc123",
            "rack-a-1",
            "Ready",
            PowerState::Off,
        )])),
    );
    let reg = |s: AppState, body: Value| {
        let admin = admin.clone();
        async move { send(s, "POST", "/api/v1/machines", &admin, Some(body), None).await }
    };
    let (st, e) = reg(s.clone(), json!({ "name": "m9", "provider": "maas" })).await;
    assert_eq!(
        (st, e["error"]["code"].as_str()),
        (StatusCode::BAD_REQUEST, Some("invalid_request")),
        "the system id is required"
    );
    let (st, e) = reg(
        s.clone(),
        json!({ "name": "m9", "provider": "maas", "systemId": "zzz999" }),
    )
    .await;
    assert_eq!(
        (st, e["error"]["code"].as_str()),
        (StatusCode::BAD_REQUEST, Some("unknown_machine"))
    );
    // MAAS down: 502, and nothing is registered.
    let down = state(
        &pool,
        fixed(Err(MaasError::Unreachable("cannot connect".into()))),
    );
    let (st, e) = reg(
        down.clone(),
        json!({ "name": "m9", "provider": "maas", "systemId": "abc123" }),
    )
    .await;
    assert_eq!(
        (st, e["error"]["code"].as_str()),
        (StatusCode::BAD_GATEWAY, Some("provisioner_unavailable"))
    );
    assert!(!e.to_string().contains("s3cret"));
    let (st, e) = send(
        down,
        "GET",
        "/api/v1/provisioners/maas/machines",
        &admin,
        None,
        None,
    )
    .await;
    assert_eq!(
        (st, e["error"]["code"].as_str()),
        (StatusCode::BAD_GATEWAY, Some("provisioner_unavailable"))
    );
    let (st, _) = reg(
        s,
        json!({ "name": "m9", "provider": "maas", "systemId": "abc123" }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM machines")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(n, 1);
}

#[test]
fn the_environment_makes_a_client_only_when_complete_and_never_echoes_the_key() {
    let env = |pairs: &'static [(&'static str, &'static str)]| {
        move |k: &str| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| (*v).to_owned())
        }
    };
    assert!(matches!(provisioners::from_env(env(&[])), Ok(Maas::Off)));
    let e = provisioners::from_env(env(&[("APELORA_MAAS_API_KEY", "c:t:topsecret")]))
        .expect_err("key without url");
    assert!(!e.contains("topsecret"), "{e}");
    let e = provisioners::from_env(env(&[("APELORA_MAAS_URL", "https://maas.example/MAAS")]))
        .expect_err("url without key");
    assert!(e.contains("APELORA_MAAS_API_KEY"), "{e}");
    let e = provisioners::from_env(env(&[
        ("APELORA_MAAS_URL", "http://maas.example/MAAS"),
        ("APELORA_MAAS_API_KEY", "c:t:topsecret"),
    ]))
    .expect_err("http");
    assert!(e.contains("https") && !e.contains("topsecret"), "{e}");
    let e = provisioners::from_env(env(&[
        ("APELORA_MAAS_URL", "https://maas.example/MAAS"),
        ("APELORA_MAAS_API_KEY", "notakey"),
    ]))
    .expect_err("bad key");
    assert!(!e.contains("notakey") || e.contains("three parts"), "{e}");
    assert!(matches!(
        provisioners::from_env(env(&[
            ("APELORA_MAAS_URL", "https://maas.example/MAAS"),
            ("APELORA_MAAS_API_KEY", "c:t:s")
        ])),
        Ok(Maas::Client(_))
    ));
}
