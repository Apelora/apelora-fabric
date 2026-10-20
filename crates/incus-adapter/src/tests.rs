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

/// Seen on Incus 7.0.1: an instance on an offline member is listed with `config: null`.
#[tokio::test]
async fn instances_on_an_unreachable_member_are_listed_with_unknown_labels() {
    let base = fixture_server(vec![(
        "/1.0/instances?recursion=1&all-projects=true",
        sync(json!([
            { "name": "shop-web-01", "project": "apelora", "type": "container", "status": "Error", "location": "node-2", "architecture": "aarch64", "description": "", "config": null, "expanded_config": null },
            { "name": "shop-web-01-r2", "project": "apelora", "type": "container", "status": "Running", "location": "node-1", "architecture": "aarch64", "description": "",
              "config": { "user.apelora.managed": "true", "user.apelora.application": "shop", "user.apelora.module": "web-01", "user.apelora.replica": "2" }, "expanded_config": {} }
        ])),
    )]).await;
    let list = Provider::Incus(IncusClient::plain_for_tests(&base))
        .instances()
        .await
        .expect("an outage must not fail the whole inventory");
    assert_eq!(
        list.iter()
            .map(|i| (i.name.as_str(), i.unreachable, i.managed))
            .collect::<Vec<_>>(),
        [
            ("shop-web-01", true, false),
            ("shop-web-01-r2", false, true)
        ]
    );
    assert_eq!(list[1].owner.as_ref().map(|o| o.replica), Some(2));
}

#[tokio::test]
async fn maps_instance_kinds_and_ignores_unknown_fields() {
    let base = fixture_server(vec![(
        "/1.0/instances?recursion=1&all-projects=true",
        sync(json!([
            { "name": "web-01", "project": "shop", "type": "container", "status": "Running", "status_code": 103, "location": "node-1", "architecture": "x86_64", "description": "", "config": { "user.apelora.managed": "true" }, "devices": {}, "a_future_field": 1 },
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
        list.iter().map(|i| i.managed).collect::<Vec<_>>(),
        vec![true, false, false],
        "only the ownership marker makes an instance Apelora's"
    );
    assert!(
        list.iter().all(|i| !i.unreachable),
        "a missing config is empty, not unknown"
    );
    assert_eq!(
        list[2].project, "",
        "missing fields default instead of failing"
    );
}

#[tokio::test]
async fn reads_projects_networks_and_pools() {
    let base = fixture_server(vec![
        ("/1.0/projects?recursion=1", sync(json!([{ "name": "default", "description": "Default Incus project", "config": {}, "used_by": [] }, { "name": "apelora", "description": "", "config": { "restricted": "true", "features.images": "true" }, "used_by": [] }]))),
        ("/1.0/networks?recursion=1&all-projects=true", sync(json!([{ "name": "incusbr0", "project": "default", "type": "bridge", "managed": true, "status": "Created", "description": "", "locations": ["node-1"], "config": {}, "used_by": [] },
            // Unmanaged host interfaces have no locations (null, as Incus 7.0.1 sends them).
            { "name": "eth0", "project": "default", "type": "physical", "managed": false, "status": "", "description": "", "locations": null, "config": {}, "used_by": [] }]))),
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
    assert_eq!(nets[0].locations, ["node-1"]);
    assert!(nets[1].locations.is_empty());
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
            fingerprint: None,
        },
        profiles: vec!["default".into()],
        config: [("limits.cpu".to_owned(), "1".to_owned())].into(),
        start: true,
        architectures: vec![],
        owner: None,
        network: None,
        volumes: Vec::new(),
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
    assert_eq!(body["profiles"], json!(["default"]));
    assert_eq!(body["config"]["limits.cpu"], "1");
    assert_eq!(body["start"], true);
}

const FP: &str = "0f5b9e1c2d3a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f9012345678";

#[tokio::test]
async fn a_pinned_image_is_requested_by_fingerprint() {
    let (base, seen) =
        recording_server(vec![("POST /1.0/instances?project=apelora", accepted())]).await;
    let p = Provider::Incus(IncusClient::plain_for_tests(&base));
    let mut spec = oci_spec();
    spec.kind = InstanceKind::VirtualMachine;
    spec.source = apelora_domain::ImageSource {
        protocol: apelora_domain::ImageProtocol::Simplestreams,
        server: "https://images.example".into(),
        alias: "debian/13".into(),
        fingerprint: Some(FP.into()),
    };
    spec.profiles = vec![];
    p.create_instance(&spec).await.expect("create");
    let (_, body) = seen.lock().expect("log")[0].clone();
    assert_eq!(body["type"], "virtual-machine");
    assert_eq!(
        body.get("profiles"),
        None,
        "absent, so Incus applies the default profile (an empty list would apply none)"
    );
    assert_eq!(
        body["source"],
        json!({ "type": "image", "protocol": "simplestreams", "server": "https://images.example",
                "alias": "debian/13", "fingerprint": FP })
    );
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
            sync(
                json!({ "name": "web-01", "status": "Running", "architecture": "aarch64",
                        "config": { "user.apelora.managed": "true", "image.os": "Debian", "volatile.base_image": FP } }),
            ),
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
            managed: true,
            architecture: "aarch64".into(),
            base_image: Some(FP.into()),
            location: String::new(),
            volume_devices: Vec::new(),
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
            managed: true,
            architecture: "x86_64".into(),
            base_image: None,
            location: "mock-member-01".into(),
            volume_devices: Vec::new(),
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

fn token_op(id: &str, member: &str, secret: &str) -> Value {
    json!({ "id": id, "class": "token", "description": "Cluster join token", "status": "Running", "status_code": 103,
            "metadata": { "serverName": member, "secret": secret, "fingerprint": "ab12",
                          "addresses": ["192.0.2.10:8443"], "expiresAt": "2026-10-20T03:00:00.123456789Z" } })
}

#[tokio::test]
async fn join_tokens_are_created_listed_and_revoked_without_exposing_secrets() {
    let other = "0d0e0f00-1111-4222-8333-444455556666";
    let (base, seen) = recording_server(vec![
        (
            "POST /1.0/cluster/members",
            (
                202,
                json!({ "type": "async", "status": "Operation created", "status_code": 100,
                        "operation": format!("/1.0/operations/{OP}"), "error_code": 0, "error": "",
                        "metadata": token_op(OP, "member-2", "s3cr3t") }),
            ),
        ),
        (
            "GET /1.0/operations?recursion=1",
            sync(json!({ "running": [
                token_op(OP, "member-2", "s3cr3t"),
                { "id": other, "class": "task", "status": "Running", "metadata": {} },
            ] })),
        ),
        (
            "DELETE /1.0/operations/5f4c3b2a-1d0e-4f9a-8b7c-6d5e4f3a2b1c",
            sync(json!({})),
        ),
    ])
    .await;
    let p = Provider::Incus(IncusClient::plain_for_tests(&base));

    let t = p.create_join_token("member-2").await.expect("token");
    assert_eq!(t.member_name, "member-2");
    assert_eq!(t.expires_at, "2026-10-20T03:00:00.123456789Z");
    use base64::Engine as _;
    let decoded: Value = serde_json::from_slice(
        &base64::engine::general_purpose::STANDARD
            .decode(&t.token)
            .expect("base64"),
    )
    .expect("json");
    assert_eq!(
        decoded,
        json!({ "server_name": "member-2", "fingerprint": "ab12", "addresses": ["192.0.2.10:8443"],
                "secret": "s3cr3t", "expires_at": "2026-10-20T03:00:00.123456789Z" }),
        "the format `incus admin init` accepts"
    );
    assert!(!format!("{t:?}").contains(&t.token));

    let pending = p.join_tokens().await.expect("list");
    assert_eq!(
        pending,
        vec![PendingJoin {
            member_name: "member-2".into(),
            expires_at: "2026-10-20T03:00:00.123456789Z".into()
        }]
    );
    assert_eq!(p.revoke_join_token("member-2").await, Ok(true));
    assert_eq!(p.revoke_join_token("member-9").await, Ok(false));
    let log = seen.lock().expect("log").clone();
    assert_eq!(log[0].1, json!({ "server_name": "member-2" }));
    assert!(
        log.iter()
            .any(|(k, _)| k == &format!("DELETE /1.0/operations/{OP}"))
    );
    assert!(
        !log.iter()
            .any(|(k, _)| k.contains(other) && k.starts_with("DELETE")),
        "other operations are never cancelled"
    );
    assert!(matches!(
        p.create_join_token("../x").await,
        Err(AdapterError::InvalidConfig(_))
    ));
}

#[tokio::test]
async fn mock_join_tokens_are_labelled_and_refuse_existing_members() {
    let p = Provider::Mock(MockIncus::default());
    let t = p.create_join_token("mock-member-04").await.expect("token");
    assert!(t.token.starts_with("mock-"));
    assert_eq!(p.join_tokens().await.expect("list").len(), 1);
    assert!(p.create_join_token("mock-member-01").await.is_err());
    assert_eq!(p.revoke_join_token("mock-member-04").await, Ok(true));
    assert!(p.join_tokens().await.expect("list").is_empty());
}

#[tokio::test]
async fn network_objects_are_created_once_never_adopted_and_addresses_are_read() {
    let (base, seen) = recording_server(vec![
        ("GET /1.0/networks/shop?project=apelora", error(404, "not found")),
        ("POST /1.0/networks?project=apelora", sync(json!({}))),
        (
            "GET /1.0/network-acls/shop-web-01?project=apelora",
            sync(json!({ "name": "shop-web-01", "config": { "user.apelora.managed": "true", "user.apelora.application": "shop" } })),
        ),
        (
            "GET /1.0/network-address-sets/shop-web-01?project=apelora",
            sync(json!({ "name": "shop-web-01", "config": {} })),
        ),
        (
            "GET /1.0/instances/shop-web-01/state?project=apelora",
            sync(json!({ "status": "Running", "network": { "eth0": { "addresses": [
                { "family": "inet", "address": "10.13.53.2", "scope": "global" },
                { "family": "inet6", "address": "fe80::1", "scope": "link" },
                { "family": "inet6", "address": "fd42::2", "scope": "global" } ] },
                "lo": { "addresses": [{ "family": "inet", "address": "127.0.0.1", "scope": "local" }] } } })),
        ),
        (
            "GET /1.0/instances/stopped/state?project=apelora",
            sync(json!({ "status": "Stopped", "network": null })),
        ),
    ])
    .await;
    let c = IncusClient::plain_for_tests(&base);
    c.ensure_network("apelora", "shop", "incusbr0", "shop")
        .await
        .expect("created");
    c.ensure_acl("apelora", "shop-web-01", "shop")
        .await
        .expect("already ours");
    assert!(
        matches!(
            c.ensure_address_set("apelora", "shop-web-01", "shop").await,
            Err(AdapterError::Api { status: 409, .. })
        ),
        "an object without Apelora's marker is never adopted"
    );
    assert_eq!(
        c.instance_addresses("apelora", "shop-web-01").await,
        Ok(vec!["10.13.53.2/32".to_owned(), "fd42::2/128".to_owned()])
    );
    assert_eq!(c.instance_addresses("apelora", "stopped").await, Ok(vec![]));
    let log = seen.lock().expect("log").clone();
    let created = log
        .iter()
        .find(|(k, _)| k.starts_with("POST /1.0/networks"))
        .expect("post");
    assert_eq!(
        created.1,
        json!({ "name": "shop", "type": "ovn", "description": "Apelora application network",
                "config": { "network": "incusbr0", "user.apelora.managed": "true", "user.apelora.application": "shop" } })
    );
}

#[tokio::test]
async fn an_attached_instance_gets_its_nic_on_the_application_network_with_its_acl() {
    let (base, seen) =
        recording_server(vec![("POST /1.0/instances?project=apelora", accepted())]).await;
    let mut spec = oci_spec();
    spec.network = Some(apelora_domain::network::attachment(
        "shop", "web-01", "incusbr0",
    ));
    IncusClient::plain_for_tests(&base)
        .create_instance(&spec)
        .await
        .expect("create");
    let (_, body) = seen.lock().expect("log")[0].clone();
    assert_eq!(
        body["devices"]["eth0"],
        json!({ "type": "nic", "name": "eth0", "network": "shop", "security.acls": "shop-web-01",
                "security.acls.default.ingress.action": "reject", "security.acls.default.egress.action": "reject" })
    );
}

#[test]
fn volume_sizes_are_read_in_the_units_incus_stores() {
    use crate::client::parse_mib;
    assert_eq!(parse_mib("256MiB"), Some(256));
    assert_eq!(parse_mib("2GiB"), Some(2048));
    assert_eq!(parse_mib("1TiB"), Some(1024 * 1024));
    assert_eq!(parse_mib("1GB"), None);
    assert_eq!(parse_mib("GiB"), None);
    assert_eq!(parse_mib(""), None);
}
