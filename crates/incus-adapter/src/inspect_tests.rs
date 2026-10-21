// Contract tests for the instance detail and log reads: the real client against a local fixture
// server answering with shapes from Incus's API specification (`doc/rest-api.yaml`, pinned
// release). Evidence level: CONTRACT VERIFIED. Log files and the console log are raw bodies,
// not envelopes.

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

/// Serves JSON envelopes for `json_routes` and raw bytes for `raw` (`path?query` keys);
/// anything else is a 404 error envelope.
async fn server(
    json_routes: Vec<(&'static str, (u16, Value))>,
    raw: Vec<(&'static str, Vec<u8>)>,
) -> String {
    let table: Arc<HashMap<String, (u16, Value)>> = Arc::new(
        json_routes
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect(),
    );
    let raw: Arc<HashMap<String, Vec<u8>>> =
        Arc::new(raw.into_iter().map(|(k, v)| (k.to_owned(), v)).collect());
    let app = Router::new().fallback(move |uri: Uri| {
        let (table, raw) = (table.clone(), raw.clone());
        async move {
            let key = uri
                .path_and_query()
                .map(|p| p.as_str().to_owned())
                .unwrap_or_default();
            if let Some(bytes) = raw.get(&key) {
                return (StatusCode::OK, bytes.clone()).into_response();
            }
            let (status, body) = table.get(&key).cloned().unwrap_or_else(|| {
                (
                    404,
                    json!({ "type": "error", "error": "not found", "error_code": 404 }),
                )
            });
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

#[tokio::test]
async fn reads_an_instance_in_detail_with_its_state_and_snapshots() {
    let instance = json!({
        "name": "web-01", "project": "apelora", "type": "virtual-machine", "status": "Running",
        "location": "member-2", "architecture": "aarch64", "description": "", "created_at": "2026-10-01T09:00:00Z",
        "last_used_at": "2026-10-20T08:00:00Z", "profiles": ["default"],
        "config": { "user.apelora.managed": "true", "user.apelora.application": "shop", "user.apelora.module": "web",
                    "volatile.base_image": "abc123", "image.description": "Debian trixie" },
        "expanded_config": { "limits.cpu": "2", "limits.memory": "2GiB", "image.description": "Debian trixie" },
        "expanded_devices": {
            "root": { "type": "disk", "path": "/", "pool": "local", "size": "10GiB" },
            "vol-data": { "type": "disk", "path": "/data", "pool": "local", "source": "web-01-data" },
            "eth0": { "type": "nic", "network": "ovn-shop" }
        },
        "unknown_field": 1
    });
    let state = json!({
        "status": "Running", "status_code": 103, "pid": 7, "processes": 31, "started_at": "2026-10-20T08:00:01Z",
        "cpu": { "usage": 5_000_000_000_i64 }, "memory": { "usage": 1024, "usage_peak": 2048, "total": 4096 },
        "disk": { "root": { "usage": 300 } },
        "network": {
            "lo": { "addresses": [{ "family": "inet", "address": "127.0.0.1", "netmask": "8", "scope": "local" }] },
            "eth0": { "addresses": [
                { "family": "inet6", "address": "fe80::1", "netmask": "64", "scope": "link" },
                { "family": "inet", "address": "10.10.0.5", "netmask": "24", "scope": "global" } ] }
        }
    });
    let snaps = json!([{ "name": "snap0", "created_at": "2026-10-19T00:00:00Z", "expires_at": "0001-01-01T00:00:00Z", "stateful": false }]);
    let base = server(
        vec![
            ("/1.0/instances/web-01?project=apelora", sync(instance)),
            ("/1.0/instances/web-01/state?project=apelora", sync(state)),
            (
                "/1.0/instances/web-01/snapshots?project=apelora&recursion=1",
                sync(snaps),
            ),
        ],
        vec![],
    )
    .await;
    let p = Provider::Incus(IncusClient::plain_for_tests(&base));
    let v = p
        .instance_view("apelora", "web-01")
        .await
        .expect("read")
        .expect("exists");
    assert_eq!(
        (v.kind, v.status.as_str(), v.location.as_str(), v.managed),
        (InstanceKind::VirtualMachine, "Running", "member-2", true)
    );
    assert_eq!(v.owner.map(|o| o.application), Some("shop".into()));
    assert_eq!(
        (
            v.limits.cpu.as_deref(),
            v.limits.memory.as_deref(),
            v.limits.root_disk.as_deref()
        ),
        (Some("2"), Some("2GiB"), Some("10GiB"))
    );
    let usage = v.usage.expect("running");
    assert_eq!(
        (
            usage.cpu_ns,
            usage.memory_bytes,
            usage.memory_total_bytes,
            usage.processes,
            usage.root_disk_bytes
        ),
        (
            Some(5_000_000_000),
            Some(1024),
            Some(4096),
            Some(31),
            Some(300)
        )
    );
    assert_eq!(v.started_at.as_deref(), Some("2026-10-20T08:00:01Z"));
    assert_eq!(
        v.addresses
            .iter()
            .map(|a| a.address.as_str())
            .collect::<Vec<_>>(),
        ["10.10.0.5", "fe80::1"],
        "loopback is left out"
    );
    assert_eq!(v.disks.len(), 2, "only disk devices");
    assert_eq!(
        v.disks
            .iter()
            .find(|d| d.device == "vol-data")
            .and_then(|d| d.volume.as_deref()),
        Some("web-01-data")
    );
    assert_eq!(v.snapshots.len(), 1);
    assert_eq!(v.snapshots[0].expires_at, None, "the zero time is no value");
    assert_eq!(v.image.fingerprint.as_deref(), Some("abc123"));
    assert_eq!(p.instance_view("apelora", "gone").await, Ok(None));
}

#[tokio::test]
async fn a_stopped_instance_without_readable_state_still_has_a_detail() {
    let instance = json!({ "name": "db", "project": "apelora", "type": "container", "status": "Stopped",
        "location": "member-1", "architecture": "x86_64", "config": {}, "expanded_config": {}, "expanded_devices": {} });
    let base = server(
        vec![
            ("/1.0/instances/db?project=apelora", sync(instance)),
            (
                "/1.0/instances/db/state?project=apelora",
                (
                    500,
                    json!({ "type": "error", "error": "agent not running", "error_code": 500 }),
                ),
            ),
        ],
        vec![],
    )
    .await;
    let p = Provider::Incus(IncusClient::plain_for_tests(&base));
    let v = p
        .instance_view("apelora", "db")
        .await
        .expect("read")
        .expect("exists");
    assert_eq!(
        (v.usage, v.addresses.len(), v.snapshots.len()),
        (None, 0, 0)
    );
    assert_eq!(v.created_at, None);
}

#[tokio::test]
async fn log_files_are_listed_by_name_and_read_from_their_end() {
    let mut big = Vec::new();
    for i in 0..2000 {
        big.extend_from_slice(format!("line {i}\n").as_bytes());
    }
    let base = server(
        vec![(
            "/1.0/instances/web-01/logs?project=apelora",
            sync(json!([
                "/1.0/instances/web-01/logs/lxc.log",
                "/1.0/instances/web-01/logs/lxc.conf",
                "/1.0/instances/web-01/logs/.."
            ])),
        )],
        vec![
            (
                "/1.0/instances/web-01/logs/lxc.log?project=apelora",
                big.clone(),
            ),
            (
                "/1.0/instances/web-01/console?project=apelora&type=log",
                b"booting\nlogin: ".to_vec(),
            ),
        ],
    )
    .await;
    let p = Provider::Incus(IncusClient::plain_for_tests(&base));
    assert_eq!(
        p.instance_logs("apelora", "web-01").await.expect("list"),
        ["lxc.conf", "lxc.log"],
        "names only; a name that could address something else is dropped"
    );
    let tail = p
        .instance_log("apelora", "web-01", "lxc.log", 64)
        .await
        .expect("read");
    assert!(tail.truncated);
    assert_eq!(tail.size, big.len() as u64);
    assert!(tail.content.ends_with("line 1999\n"));
    assert!(
        tail.content.starts_with("line "),
        "starts at a whole line: {:?}",
        tail.content
    );
    assert!(tail.content.len() <= 64);
    assert!(matches!(
        p.instance_log("apelora", "web-01", "../../1.0", 64).await,
        Err(AdapterError::InvalidConfig(_))
    ));
    assert!(matches!(
        p.instance_log("apelora", "web-01", "missing.log", 64).await,
        Err(AdapterError::NotFound(_))
    ));
    let console = p
        .console_log("apelora", "web-01", 1024)
        .await
        .expect("console");
    assert_eq!(
        (console.content.as_str(), console.truncated),
        ("booting\nlogin: ", false)
    );
}

#[tokio::test]
async fn mock_logs_and_detail_are_labelled_fixtures() {
    let p = Provider::Mock(MockIncus::default());
    let v = p
        .instance_view("default", "mock-vm-01")
        .await
        .expect("read")
        .expect("exists");
    assert_eq!(v.kind, InstanceKind::VirtualMachine);
    assert_eq!(
        p.instance_logs("default", "mock-vm-01")
            .await
            .expect("list"),
        ["qemu.log"]
    );
    let log = p
        .instance_log("default", "mock-vm-01", "qemu.log", 4096)
        .await
        .expect("read");
    assert!(log.content.starts_with("Mock · "));
    assert!(
        p.console_log("default", "mock-web-01", 4096)
            .await
            .expect("console")
            .content
            .starts_with("Mock · ")
    );
    assert!(matches!(
        p.instance_logs("default", "nope").await,
        Err(AdapterError::NotFound(_))
    ));
}
