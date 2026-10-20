// Contract tests: the real client against a local fixture server that answers with
// response shapes from Incus's API specification. Evidence level: CONTRACT VERIFIED
// (spec-shaped fixtures), not a real Incus server.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Router;
use axum::http::{StatusCode, Uri};
use axum::response::IntoResponse;
use serde_json::{Value, json};

use super::*;

fn sync(metadata: Value) -> (u16, Value) {
    (
        200,
        json!({ "type": "sync", "status": "Success", "status_code": 200, "operation": "", "error_code": 0, "error": "", "metadata": metadata }),
    )
}

fn error(code: u16, msg: &str) -> (u16, Value) {
    (
        code,
        json!({ "type": "error", "error": msg, "error_code": code }),
    )
}

/// Serves `path?query` -> (status, body); anything else is a 404 error envelope.
async fn fixture_server(routes: Vec<(&'static str, (u16, Value))>) -> String {
    let table: Arc<HashMap<String, (u16, Value)>> =
        Arc::new(routes.into_iter().map(|(k, v)| (k.to_owned(), v)).collect());
    let app = Router::new().fallback(move |uri: Uri| {
        let table = table.clone();
        async move {
            let key = uri
                .path_and_query()
                .map(|p| p.as_str().to_owned())
                .unwrap_or_default();
            let (status, body) = table
                .get(&key)
                .cloned()
                .unwrap_or_else(|| error(404, "not found"));
            (
                StatusCode::from_u16(status).expect("status"),
                axum::Json(body),
            )
                .into_response()
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    format!("http://{addr}")
}

fn server_json(clustered: bool) -> Value {
    json!({
        "api_extensions": ["storage_zfs_remove_snapshots", "clustering", "instances"],
        "api_status": "stable", "api_version": "1.0", "auth": "trusted", "auth_methods": ["tls"],
        "config": {}, "public": false,
        "environment": { "server_clustered": clustered, "server_version": "7.0.2", "server_name": "node-1", "kernel_version": "6.12.0", "storage": "zfs" }
    })
}

#[tokio::test]
async fn reads_server_info_and_extensions() {
    let base = fixture_server(vec![("/1.0", sync(server_json(true)))]).await;
    let p = Provider::Incus(IncusClient::plain_for_tests(&base));
    let info = p.server_info().await.expect("info");
    assert_eq!(info.server_version.as_deref(), Some("7.0.2"));
    assert_eq!(info.kernel_version.as_deref(), Some("6.12.0"));
    assert!(info.clustered);
    assert_eq!(info.auth, "trusted");
    assert_eq!(p.require_extension("clustering").await, Ok(()));
    assert_eq!(
        p.require_extension("no_such_extension").await,
        Err(AdapterError::UnsupportedCapability(
            "no_such_extension".into()
        ))
    );
}

#[tokio::test]
async fn maps_cluster_members() {
    let base = fixture_server(vec![
        ("/1.0", sync(server_json(true))),
        ("/1.0/cluster/members?recursion=1", sync(json!([
            { "server_name": "node-1", "url": "https://10.0.0.1:8443", "roles": ["database-leader", "database"], "status": "Online", "message": "Fully operational", "architecture": "x86_64", "failure_domain": "rack-a", "database": true, "groups": ["default"], "config": {}, "description": "" },
            { "server_name": "node-2", "url": "https://10.0.0.2:8443", "roles": ["database"], "status": "Offline", "message": "No heartbeat", "architecture": "aarch64", "failure_domain": "rack-b", "database": true, "groups": [], "config": {}, "description": "" }
        ]))),
    ]).await;
    let members = Provider::Incus(IncusClient::plain_for_tests(&base))
        .cluster_members()
        .await
        .expect("members");
    assert_eq!(members.len(), 2);
    assert_eq!(members[0].name, "node-1");
    assert_eq!(members[0].roles, vec!["database-leader", "database"]);
    assert_eq!(members[1].status, "Offline");
    assert_eq!(members[1].failure_domain, "rack-b");
}

#[tokio::test]
async fn standalone_server_has_no_members_and_skips_the_cluster_call() {
    // No /1.0/cluster/members route: calling it would 404.
    let base = fixture_server(vec![("/1.0", sync(server_json(false)))]).await;
    let members = Provider::Incus(IncusClient::plain_for_tests(&base))
        .cluster_members()
        .await
        .expect("members");
    assert!(members.is_empty());
}

#[tokio::test]
async fn maps_instance_kinds_and_ignores_unknown_fields() {
    let base = fixture_server(vec![(
        "/1.0/instances?recursion=1&all-projects=true",
        sync(json!([
            { "name": "web-01", "project": "shop", "type": "container", "status": "Running", "status_code": 103, "location": "node-1", "architecture": "x86_64", "description": "", "config": {}, "devices": {}, "a_future_field": 1 },
            { "name": "win-01", "project": "default", "type": "virtual-machine", "status": "Stopped", "status_code": 102, "location": "node-2", "architecture": "x86_64", "description": "" },
            { "name": "odd-01", "type": "something-new" }
        ])),
    )]).await;
    let list = Provider::Incus(IncusClient::plain_for_tests(&base))
        .instances()
        .await
        .expect("instances");
    assert_eq!(
        list.iter().map(|i| i.kind).collect::<Vec<_>>(),
        vec![
            InstanceKind::Container,
            InstanceKind::VirtualMachine,
            InstanceKind::Unknown
        ]
    );
    assert_eq!(list[0].project, "shop");
    assert_eq!(
        list[2].project, "",
        "missing fields default instead of failing"
    );
}

#[tokio::test]
async fn reads_projects_networks_and_pools() {
    let base = fixture_server(vec![
        ("/1.0/projects?recursion=1", sync(json!([{ "name": "default", "description": "Default Incus project", "config": {}, "used_by": [] }, { "name": "apelora", "description": "", "config": { "restricted": "true", "features.images": "true" }, "used_by": [] }]))),
        ("/1.0/networks?recursion=1&all-projects=true", sync(json!([{ "name": "incusbr0", "project": "default", "type": "bridge", "managed": true, "status": "Created", "description": "", "locations": ["node-1"], "config": {}, "used_by": [] }]))),
        ("/1.0/storage-pools?recursion=1", sync(json!([{ "name": "local", "driver": "zfs", "status": "Created", "description": "", "locations": ["node-1"], "config": {}, "used_by": [] }]))),
    ]).await;
    let p = Provider::Incus(IncusClient::plain_for_tests(&base));
    let projects = p.projects().await.expect("projects");
    assert_eq!(
        projects
            .iter()
            .map(|p| (p.name.as_str(), p.restricted))
            .collect::<Vec<_>>(),
        [("default", false), ("apelora", true)]
    );
    let nets = p.networks().await.expect("networks");
    assert_eq!((nets[0].kind.as_str(), nets[0].managed), ("bridge", true));
    assert_eq!(p.storage_pools().await.expect("pools")[0].driver, "zfs");
}

#[tokio::test]
async fn operations_accept_flat_and_grouped_shapes() {
    let op = |id: &str, status: &str| json!({ "id": id, "class": "task", "description": "Creating instance", "status": status, "status_code": 103, "err": "", "location": "node-1", "may_cancel": false, "created_at": "2026-10-20T00:00:00Z", "updated_at": "2026-10-20T00:00:01Z", "resources": {}, "metadata": {} });
    let flat = fixture_server(vec![(
        "/1.0/operations?recursion=1&all-projects=true",
        sync(json!([op("a", "Running")])),
    )])
    .await;
    assert_eq!(
        IncusClient::plain_for_tests(&flat)
            .operations()
            .await
            .expect("ops")[0]
            .id,
        "a"
    );
    let grouped = fixture_server(vec![(
        "/1.0/operations?recursion=1&all-projects=true",
        sync(json!({ "running": [op("b", "Running")], "success": [op("c", "Success")] })),
    )])
    .await;
    let mut ids: Vec<String> = IncusClient::plain_for_tests(&grouped)
        .operations()
        .await
        .expect("ops")
        .into_iter()
        .map(|o| o.id)
        .collect();
    ids.sort();
    assert_eq!(ids, vec!["b", "c"]);
}

#[tokio::test]
async fn error_envelopes_become_typed_errors() {
    let base = fixture_server(vec![
        ("/1.0", error(403, "not authorized")),
        (
            "/1.0/projects?recursion=1",
            error(500, "database is locked"),
        ),
    ])
    .await;
    let p = Provider::Incus(IncusClient::plain_for_tests(&base));
    assert_eq!(
        p.server_info().await,
        Err(AdapterError::Authentication("not authorized".into()))
    );
    assert_eq!(
        p.projects().await,
        Err(AdapterError::Api {
            status: 500,
            message: "database is locked".into()
        })
    );
    assert!(matches!(
        p.storage_pools().await,
        Err(AdapterError::NotFound(_))
    ));
}

#[tokio::test]
async fn unreachable_server_is_a_connectivity_error() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    drop(listener);
    let res = IncusClient::plain_for_tests(&format!("http://{addr}"))
        .server_info()
        .await;
    assert!(matches!(res, Err(AdapterError::Connectivity(_))), "{res:?}");
}

#[test]
fn mtls_refuses_plain_http_and_bad_certificates() {
    let cfg = IncusConfig {
        url: "http://incus.invalid:8443".into(),
        client_cert_pem: vec![],
        client_key_pem: vec![],
        server_cert_pem: vec![],
        tls_server_name: None,
    };
    assert!(matches!(
        IncusClient::mtls(&cfg),
        Err(AdapterError::InvalidConfig(_))
    ));
    let cfg = IncusConfig {
        url: "https://incus.invalid:8443".into(),
        client_cert_pem: b"not a cert".to_vec(),
        client_key_pem: b"not a key".to_vec(),
        server_cert_pem: b"nope".to_vec(),
        tls_server_name: None,
    };
    assert!(matches!(
        IncusClient::mtls(&cfg),
        Err(AdapterError::InvalidConfig(_))
    ));
    assert!(
        !format!("{cfg:?}").contains("not a key"),
        "debug output must not reveal key material"
    );
}

#[tokio::test]
async fn mock_is_labelled_and_claims_no_version() {
    let p = Provider::Mock(MockIncus::default());
    assert_eq!(p.mode(), ProviderMode::Mock);
    let info = p.server_info().await.expect("info");
    assert_eq!(info.server_version, None);
    assert!(
        p.cluster_members()
            .await
            .expect("members")
            .iter()
            .all(|m| m.name.starts_with("mock-"))
    );
    assert!(
        p.instances()
            .await
            .expect("instances")
            .iter()
            .all(|i| i.name.starts_with("mock-"))
    );
}

// ---- Lifecycle writes -------------------------------------------------------------------

type Recorded = Arc<std::sync::Mutex<Vec<(String, Value)>>>;

/// Routes on "METHOD path?query" and records every request with its JSON body.
async fn recording_server(routes: Vec<(&'static str, (u16, Value))>) -> (String, Recorded) {
    let table: Arc<HashMap<String, (u16, Value)>> =
        Arc::new(routes.into_iter().map(|(k, v)| (k.to_owned(), v)).collect());
    let seen: Recorded = Arc::default();
    let log = seen.clone();
    let app = Router::new().fallback(
        move |method: axum::http::Method, uri: Uri, body: axum::body::Bytes| {
            let (table, log) = (table.clone(), log.clone());
            async move {
                let key = format!(
                    "{method} {}",
                    uri.path_and_query().map(|p| p.as_str()).unwrap_or_default()
                );
                log.lock().expect("log").push((
                    key.clone(),
                    serde_json::from_slice(&body).unwrap_or(Value::Null),
                ));
                let (status, body) = table
                    .get(&key)
                    .cloned()
                    .unwrap_or_else(|| error(404, "not found"));
                (
                    StatusCode::from_u16(status).expect("status"),
                    axum::Json(body),
                )
                    .into_response()
            }
        },
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    (format!("http://{addr}"), seen)
}

const OP: &str = "5f4c3b2a-1d0e-4f9a-8b7c-6d5e4f3a2b1c";

fn accepted() -> (u16, Value) {
    (
        202,
        json!({ "type": "async", "status": "Operation created", "status_code": 100,
                "operation": format!("/1.0/operations/{OP}"), "error_code": 0, "error": "",
                "metadata": { "id": OP, "class": "task", "status": "Running", "status_code": 103 } }),
    )
}

fn operation(status: &str, err: &str) -> (u16, Value) {
    sync(
        json!({ "id": OP, "class": "task", "description": "Creating instance", "status": status, "err": err, "may_cancel": false }),
    )
}

fn oci_spec() -> InstanceSpec {
    InstanceSpec {
        project: "apelora".into(),
        name: "web-01".into(),
        kind: InstanceKind::Container,
        source: apelora_domain::ImageSource {
            protocol: apelora_domain::ImageProtocol::Oci,
            server: "https://registry.example".into(),
            alias: "nginx:1.29".into(),
        },
        profiles: vec!["default".into()],
        config: [("limits.cpu".to_owned(), "1".to_owned())].into(),
        start: true,
    }
}

#[tokio::test]
async fn create_sends_the_spec_with_the_ownership_marker_and_returns_the_operation() {
    let (base, seen) = recording_server(vec![
        ("POST /1.0/instances?project=apelora", accepted()),
        (
            "GET /1.0/operations/5f4c3b2a-1d0e-4f9a-8b7c-6d5e4f3a2b1c/wait?timeout=15",
            operation("Success", ""),
        ),
    ])
    .await;
    let p = Provider::Incus(IncusClient::plain_for_tests(&base));
    let op = p.create_instance(&oci_spec()).await.expect("create");
    assert_eq!(op, OP);
    assert_eq!(
        p.wait_operation(&op, std::time::Duration::from_secs(60))
            .await,
        Ok(OperationOutcome::Succeeded)
    );
    let (_, body) = seen.lock().expect("log")[0].clone();
    assert_eq!(body["name"], "web-01");
    assert_eq!(body["type"], "container");
    assert_eq!(
        body["source"],
        json!({ "type": "image", "protocol": "oci", "server": "https://registry.example", "alias": "nginx:1.29" })
    );
    assert_eq!(body["config"]["user.apelora.managed"], "true");
    assert_eq!(body["config"]["limits.cpu"], "1");
    assert_eq!(body["start"], true);
}

#[tokio::test]
async fn invalid_specs_never_reach_incus() {
    let (base, seen) = recording_server(vec![]).await;
    let p = Provider::Incus(IncusClient::plain_for_tests(&base));
    let bad = InstanceSpec {
        name: "../../1.0/cluster".into(),
        ..oci_spec()
    };
    assert!(matches!(
        p.create_instance(&bad).await,
        Err(AdapterError::InvalidConfig(_))
    ));
    assert!(matches!(
        p.wait_operation("../evil", std::time::Duration::from_secs(1))
            .await,
        Err(AdapterError::InvalidConfig(_))
    ));
    assert!(seen.lock().expect("log").is_empty());
}

#[tokio::test]
async fn reads_ownership_and_reports_failed_operations() {
    let (base, seen) = recording_server(vec![
        (
            "GET /1.0/instances/web-01?project=apelora",
            sync(json!({ "name": "web-01", "status": "Running", "config": { "user.apelora.managed": "true", "image.os": "Debian" } })),
        ),
        (
            "GET /1.0/instances/theirs?project=apelora",
            sync(json!({ "name": "theirs", "status": "Running", "config": {} })),
        ),
        ("PUT /1.0/instances/web-01/state?project=apelora", accepted()),
        ("DELETE /1.0/instances/web-01?project=apelora", accepted()),
        (
            "GET /1.0/operations/5f4c3b2a-1d0e-4f9a-8b7c-6d5e4f3a2b1c/wait?timeout=15",
            operation("Failure", "Failed to stop instance"),
        ),
    ])
    .await;
    let p = Provider::Incus(IncusClient::plain_for_tests(&base));
    assert_eq!(
        p.instance("apelora", "web-01").await,
        Ok(Some(InstanceState {
            status: "Running".into(),
            managed: true
        }))
    );
    assert_eq!(
        p.instance("apelora", "theirs")
            .await
            .map(|s| s.map(|s| s.managed)),
        Ok(Some(false))
    );
    assert_eq!(p.instance("apelora", "gone").await, Ok(None));
    let op = p
        .change_state("apelora", "web-01", InstanceAction::Stop, true)
        .await
        .expect("stop");
    assert_eq!(
        p.wait_operation(&op, std::time::Duration::from_secs(60))
            .await,
        Ok(OperationOutcome::Failed("Failed to stop instance".into()))
    );
    assert_eq!(
        p.delete_instance("apelora", "web-01").await.as_deref(),
        Ok(OP)
    );
    let log = seen.lock().expect("log").clone();
    let state = log
        .iter()
        .find(|(k, _)| k.starts_with("PUT"))
        .expect("state request");
    assert_eq!(
        state.1,
        json!({ "action": "stop", "timeout": 30, "force": true })
    );
}

#[tokio::test]
async fn a_sync_reply_to_a_write_is_not_mistaken_for_an_operation() {
    let (base, _) = recording_server(vec![(
        "DELETE /1.0/instances/web-01?project=apelora",
        sync(json!({})),
    )])
    .await;
    let p = Provider::Incus(IncusClient::plain_for_tests(&base));
    assert!(matches!(
        p.delete_instance("apelora", "web-01").await,
        Err(AdapterError::Decode(_))
    ));
}

#[tokio::test]
async fn mock_lifecycle_is_shared_between_clones_and_marks_ownership() {
    let a = Provider::Mock(MockIncus::default());
    let b = a.clone();
    a.create_instance(&InstanceSpec {
        project: "default".into(),
        ..oci_spec()
    })
    .await
    .expect("create");
    assert_eq!(
        b.instance("default", "web-01").await,
        Ok(Some(InstanceState {
            status: "Running".into(),
            managed: true
        }))
    );
    assert_eq!(
        b.instance("default", "mock-web-01")
            .await
            .map(|s| s.map(|s| s.managed)),
        Ok(Some(false)),
        "seeded instances are not Apelora's"
    );
    assert!(matches!(
        b.delete_instance("default", "web-01").await,
        Err(AdapterError::Api { status: 400, .. })
    ));
    b.change_state("default", "web-01", InstanceAction::Stop, false)
        .await
        .expect("stop");
    b.delete_instance("default", "web-01")
        .await
        .expect("delete");
    assert_eq!(a.instance("default", "web-01").await, Ok(None));
    assert_eq!(a.instances().await.map(|v| v.len()), Ok(4));
}
