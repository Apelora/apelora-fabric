// Health and capacity through the real router: mock provider (MOCK VERIFIED) and a real
// PostgreSQL for heartbeats, the job queue and permissions.

use apelora_domain::capacity::{HealthPoolUsage, HealthServerResources, HealthUsage};
use apelora_incus_adapter::{IncusClient, MockIncus};
use serde_json::json;

use super::identity_tests::{code, lab, world};
use super::tests::send;
use super::*;

const GIB: u64 = 1024 * 1024 * 1024;

fn dev(mock: &MockIncus) -> AppState {
    AppState::new(Provider::Mock(mock.clone()), Auth::DisabledForDevelopment)
}

async fn get(state: &AppState, path: &str) -> (StatusCode, Value) {
    send(
        state.clone(),
        "GET",
        path,
        "unused-in-development",
        None,
        None,
    )
    .await
}

#[tokio::test]
async fn development_mode_reports_every_component_honestly() {
    let (status, h) = get(&dev(&MockIncus::default()), "/api/v1/health/components").await;
    assert_eq!(status, StatusCode::OK, "{h}");
    assert!(h["checkedAt"].as_str().is_some_and(|t| t.ends_with('Z')));
    // Sign-in off is a real risk; the missing database and workers are deliberate.
    assert_eq!(h["controlPlane"]["state"], "degraded");
    assert_eq!(h["controlPlane"]["auth"], "disabled-development");
    assert_eq!(h["controlPlane"]["provider"], "mock");
    assert_eq!(h["database"]["state"], "not-set-up");
    assert_eq!(h["workers"]["state"], "not-set-up");
    assert_eq!(h["jobQueue"]["state"], "not-set-up");
    assert_eq!(h["jobQueue"]["counts"], Value::Null);
    // Mock data is never reported as a healthy Incus.
    assert_eq!(h["incus"]["state"], "not-set-up");
    assert!(
        h["incus"]["reason"]
            .as_str()
            .is_some_and(|r| r.starts_with("Mock data"))
    );
    assert_eq!(h["incus"]["members"]["online"], 3);
    assert_eq!(h["networks"]["state"], "healthy");
    assert_eq!(h["networks"]["total"], 1);
    assert_eq!(h["storagePools"]["total"], 1);
    assert_eq!(h["identityProviders"]["state"], "not-set-up");
    assert_eq!(h["overall"], "degraded");
}

#[tokio::test]
async fn servers_show_resources_instances_and_warnings() {
    let mock = MockIncus::default();
    let state = dev(&mock);
    let (status, s) = get(&state, "/api/v1/health/servers").await;
    assert_eq!(status, StatusCode::OK, "{s}");
    assert_eq!(s["source"]["provider"], "mock");
    let items = s["items"].as_array().expect("items");
    assert_eq!(items.len(), 3);
    assert_eq!(items[0]["name"], "mock-member-01");
    assert_eq!(items[0]["state"], "healthy");
    assert_eq!(items[0]["resources"]["cpuThreads"], 8);
    assert_eq!(items[0]["resources"]["memory"]["totalBytes"], 32 * GIB);
    // Instances are listed where Incus says they run.
    assert_eq!(items[1]["instances"].as_array().map(Vec::len), Some(2));

    // A full server and an offline one: the server decides the warnings.
    mock.resources.lock().expect("lock").insert(
        "mock-member-01".into(),
        HealthServerResources {
            cpu_threads: Some(4),
            load_averages: vec![0.5],
            memory: Some(HealthUsage {
                used_bytes: 31 * GIB,
                total_bytes: 32 * GIB,
            }),
            pools: vec![HealthPoolUsage {
                name: "mock-local".into(),
                used_bytes: 90,
                total_bytes: 100,
            }],
            uptime_secs: None,
        },
    );
    mock.members.lock().expect("lock")[2].status = "Offline".into();
    let (_, s) = get(&state, "/api/v1/health/servers").await;
    let warnings: Vec<&str> = s["items"][0]["warnings"]
        .as_array()
        .expect("warnings")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(s["items"][0]["state"], "degraded");
    assert!(warnings[0].starts_with("Memory 97 % used"), "{warnings:?}");
    assert!(warnings[1].starts_with("Storage pool mock-local 90 % full"));
    assert_eq!(s["items"][2]["state"], "down");
    assert_eq!(s["items"][2]["resources"], Value::Null);
    assert!(
        s["items"][2]["resourcesError"]
            .as_str()
            .is_some_and(|e| e.contains("does not answer"))
    );
    let (_, h) = get(&state, "/api/v1/health/components").await;
    assert_eq!(h["incus"]["state"], "not-set-up", "mock stays mock");
    assert_eq!(h["incus"]["members"]["offline"], 1);
}

#[tokio::test]
async fn capacity_sums_only_servers_that_accept_modules() {
    let mock = MockIncus::default();
    mock.members.lock().expect("lock")[1].status = "Evacuated".into();
    mock.members.lock().expect("lock")[2].scheduler = "manual".into();
    let state = dev(&mock).with_volume_pool(Some("mock-local".into()));
    let (status, c) = get(&state, "/api/v1/capacity").await;
    assert_eq!(status, StatusCode::OK, "{c}");
    assert_eq!(
        c["named"], true,
        "development mode acts as an administrator"
    );
    assert_eq!(c["volumePool"], "mock-local");
    assert_eq!(c["servers"][0]["acceptsModules"], true);
    assert_eq!(c["servers"][1]["reason"], "It is Evacuated");
    assert_eq!(c["servers"][2]["reason"], "Held out of automatic placement");
    assert_eq!(c["totals"]["serversAccepting"], 1);
    assert_eq!(c["totals"]["cpuThreads"], 8);
    assert_eq!(c["totals"]["memoryFreeBytes"], 23 * GIB);
    assert_eq!(c["totals"]["volumePoolFreeBytes"], 380 * GIB);
    assert_eq!(c["servers"][0]["volumePool"]["totalBytes"], 500 * GIB);
    assert_eq!(c["volumePoolShared"], false);
    // Without a volume pool there is nothing to compare volumes with.
    let (_, c) = get(&dev(&mock), "/api/v1/capacity").await;
    assert_eq!(c["totals"]["volumePoolFreeBytes"], Value::Null);
    assert_eq!(c["servers"][0]["volumePool"], Value::Null);
}

#[tokio::test]
async fn a_shared_volume_pool_is_counted_once() {
    let mut mock = MockIncus::default();
    mock.pools[0].driver = "lvmcluster".into();
    let state = dev(&mock).with_volume_pool(Some("mock-local".into()));
    let (_, c) = get(&state, "/api/v1/capacity").await;
    assert_eq!(c["volumePoolShared"], true);
    // Not 1,090 GiB (three servers' sums): one pool, as large as one server sees it.
    assert_eq!(c["totals"]["volumePoolFreeBytes"], 420 * GIB);
}

#[tokio::test]
async fn unreachable_incus_is_down_not_empty() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    drop(listener);
    let state = AppState::new(
        Provider::Incus(IncusClient::plain_for_tests(&format!("http://{addr}"))),
        Auth::DisabledForDevelopment,
    );
    let (status, h) = get(&state, "/api/v1/health/components").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(h["incus"]["state"], "down");
    assert!(
        h["incus"]["reason"]
            .as_str()
            .is_some_and(|r| r.contains("cannot reach"))
    );
    assert_eq!(h["networks"]["state"], "unknown");
    assert_eq!(h["networks"]["total"], Value::Null);
    assert_eq!(h["overall"], "down");
    let (status, s) = get(&state, "/api/v1/health/servers").await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(s["error"]["code"], "incus_unreachable");
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn database_workers_and_queue_come_from_postgresql(pool: Pool) {
    let l = lab(pool.clone());
    let (_, admin) = l.super_admin("sub-root", "root").await;
    let (status, h) = l
        .req("GET", "/api/v1/health/components", &admin, None)
        .await;
    assert_eq!(status, StatusCode::OK, "{h}");
    assert_eq!(h["controlPlane"]["state"], "healthy");
    assert_eq!(h["database"]["state"], "healthy");
    assert_eq!(
        h["database"]["schemaVersion"],
        h["database"]["expectedSchemaVersion"]
    );
    assert!(h["database"]["roundTripMs"].as_f64().is_some());
    // No worker has ever run: nothing would be deployed.
    assert_eq!(h["workers"]["state"], "down");
    assert_eq!(h["jobQueue"]["state"], "healthy");
    // The environment's identity provider is configured in the lab.
    assert_eq!(h["identityProviders"]["state"], "healthy");
    assert_eq!(h["identityProviders"]["items"][0]["source"], "environment");

    // A job waits and no worker runs: the queue is down and says why.
    apelora_persistence::enqueue_job(&pool, "noop", None, &json!({}), None)
        .await
        .expect("enqueue");
    let (_, h) = l
        .req("GET", "/api/v1/health/components", &admin, None)
        .await;
    assert_eq!(h["jobQueue"]["state"], "down");
    assert_eq!(h["jobQueue"]["counts"]["waiting"], 1);
    assert_eq!(
        h["jobQueue"]["reason"],
        "1 job wait and no worker is running"
    );

    // A worker starts.
    apelora_persistence::status::worker_started(&pool, "worker-1", "host-a", "0.0.0")
        .await
        .expect("start");
    let (_, h) = l
        .req("GET", "/api/v1/health/components", &admin, None)
        .await;
    assert_eq!(h["workers"]["state"], "healthy");
    assert_eq!(h["workers"]["items"][0]["state"], "running");
    assert_eq!(h["workers"]["items"][0]["host"], "host-a");
    assert_eq!(h["jobQueue"]["state"], "healthy");

    // It stops sending heartbeats without shutting down.
    sqlx::query("UPDATE worker_heartbeats SET last_seen = now() - interval '2 minutes'")
        .execute(&pool)
        .await
        .expect("age");
    let (_, h) = l
        .req("GET", "/api/v1/health/components", &admin, None)
        .await;
    assert_eq!(h["workers"]["state"], "down");
    assert_eq!(h["workers"]["items"][0]["state"], "lost");
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn health_needs_cluster_view_and_designers_see_numbered_capacity(pool: Pool) {
    let l = lab(pool);
    let w = world(&l).await;
    let des = l.user("sub-des", "des").await;
    let (s, g) = l
        .grant(
            &w.admin,
            des,
            "application-designer",
            json!({ "type": "project", "id": w.acme_prod }),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{g}");
    let td = l.token("sub-des", "des");
    for path in ["/api/v1/health/components", "/api/v1/health/servers"] {
        let (status, body) = l.req("GET", path, &td, None).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}: {body}");
        assert_eq!(code(&body), Some("forbidden"));
    }
    let (status, c) = l.req("GET", "/api/v1/capacity", &td, None).await;
    assert_eq!(status, StatusCode::OK, "{c}");
    assert_eq!(c["named"], false);
    let labels: Vec<&str> = c["servers"]
        .as_array()
        .expect("servers")
        .iter()
        .filter_map(|s| s["label"].as_str())
        .collect();
    assert_eq!(labels, ["Server 1", "Server 2", "Server 3"]);
    assert!(
        !c.to_string().contains("mock-member"),
        "no server names leak: {c}"
    );

    // A person with no role sees nothing.
    let nobody = l.token("sub-none", "nobody");
    l.user("sub-none", "nobody").await;
    for path in [
        "/api/v1/health/components",
        "/api/v1/health/servers",
        "/api/v1/capacity",
    ] {
        assert_eq!(
            l.req("GET", path, &nobody, None).await.0,
            StatusCode::FORBIDDEN,
            "{path}"
        );
    }

    // A viewer at platform level holds cluster.view: everything, with names.
    let viewer = l.user("sub-view", "viewer").await;
    l.grant(&w.admin, viewer, "viewer", json!({ "type": "platform" }))
        .await;
    let tv = l.token("sub-view", "viewer");
    for path in ["/api/v1/health/components", "/api/v1/health/servers"] {
        assert_eq!(
            l.req("GET", path, &tv, None).await.0,
            StatusCode::OK,
            "{path}"
        );
    }
    let (_, c) = l.req("GET", "/api/v1/capacity", &tv, None).await;
    assert_eq!(c["named"], true);
    assert_eq!(c["servers"][0]["label"], "mock-member-01");

    // An organisation owner (tenant permissions only) gets numbered capacity, not health.
    let owner = l.user("sub-own", "owner").await;
    l.grant(
        &w.admin,
        owner,
        "organization-owner",
        json!({ "type": "organization", "id": w.other }),
    )
    .await;
    let to = l.token("sub-own", "owner");
    assert_eq!(
        l.req("GET", "/api/v1/health/servers", &to, None).await.0,
        StatusCode::FORBIDDEN
    );
    let (status, c) = l.req("GET", "/api/v1/capacity", &to, None).await;
    assert_eq!(
        (status, c["named"].as_bool()),
        (StatusCode::OK, Some(false))
    );
    let _ = w.other_proj;
}
