use super::*;
use apelora_incus_adapter::{IncusClient, MockIncus};
use axum::body::Body;
use axum::http::Request;
use http_body_util::BodyExt;
use serde_json::json;
use tower::ServiceExt;

async fn call(
    state: AppState,
    path: &str,
    token: Option<&str>,
) -> (StatusCode, Value, Option<String>) {
    let mut req = Request::get(path);
    if let Some(t) = token {
        req = req.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let res = router(state)
        .oneshot(req.body(Body::empty()).expect("request"))
        .await
        .expect("response");
    let status = res.status();
    let www = res
        .headers()
        .get(header::WWW_AUTHENTICATE)
        .map(|v| v.to_str().unwrap_or_default().to_owned());
    let bytes = res.into_body().collect().await.expect("body").to_bytes();
    (status, serde_json::from_slice(&bytes).expect("json"), www)
}

fn dev(provider: Provider) -> AppState {
    AppState::new(provider, Auth::DisabledForDevelopment)
}

/// `docs/architecture/SETTINGS.md` is generated from the settings list. Regenerate with
/// `npm run settings:doc` (or `UPDATE_SETTINGS_DOC=1 cargo test -p apelora-control-plane`).
#[test]
fn committed_settings_document_is_current() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/architecture/SETTINGS.md"
    );
    let current = super::settings::markdown();
    if std::env::var_os("UPDATE_SETTINGS_DOC").is_some() {
        std::fs::write(path, &current).expect("write SETTINGS.md");
    }
    let committed = std::fs::read_to_string(path).unwrap_or_default();
    assert!(
        committed == current,
        "{path} is stale: run `npm run settings:doc`"
    );
}

#[tokio::test]
async fn health_reports_mock_provider_and_disabled_auth_in_development() {
    let (status, body, _) = call(
        dev(Provider::Mock(MockIncus::default())),
        "/api/v1/health",
        None,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body["provider"], "mock");
    assert_eq!(body["auth"], "disabled-development");
    assert_eq!(body["incus"]["connected"], false);
}

#[tokio::test]
async fn inventory_endpoints_label_their_source() {
    for path in [
        "/api/v1/cluster",
        "/api/v1/instances",
        "/api/v1/projects",
        "/api/v1/networks",
        "/api/v1/storage-pools",
    ] {
        let (status, body, _) = call(dev(Provider::Mock(MockIncus::default())), path, None).await;
        assert_eq!(status, 200, "{path}");
        assert_eq!(body["source"]["provider"], "mock", "{path}");
        assert!(
            body["source"]["observedAt"]
                .as_str()
                .is_some_and(|t| t.ends_with('Z')),
            "{path}: {body}"
        );
    }
    let (_, cluster, _) = call(
        dev(Provider::Mock(MockIncus::default())),
        "/api/v1/cluster",
        None,
    )
    .await;
    assert_eq!(cluster["members"].as_array().map(Vec::len), Some(3));
    let (_, inst, _) = call(
        dev(Provider::Mock(MockIncus::default())),
        "/api/v1/instances",
        None,
    )
    .await;
    assert_eq!(inst["items"][3]["kind"], "virtual-machine");
}

#[tokio::test]
async fn unreachable_incus_is_a_502_with_a_stable_code() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    drop(listener);
    let provider = Provider::Incus(IncusClient::plain_for_tests(&format!("http://{addr}")));
    let (status, body, _) = call(dev(provider.clone()), "/api/v1/instances", None).await;
    assert_eq!(status, 502);
    assert_eq!(body["error"]["code"], "incus_unreachable");
    let (status, body, _) = call(dev(provider), "/api/v1/health", None).await;
    assert_eq!(status, 200);
    assert_eq!(body["incus"]["connected"], false);
}

#[tokio::test]
async fn audit_needs_a_database_in_development_mode() {
    let (status, body, _) = call(
        dev(Provider::Mock(MockIncus::default())),
        "/api/v1/audit",
        None,
    )
    .await;
    assert_eq!(status, 503);
    assert_eq!(body["error"]["code"], "database_not_configured");
}

/// Sends a GET with an optional `x-request-id`; returns the status and the echoed id.
async fn with_request_id(state: AppState, path: &str, id: Option<&str>) -> (StatusCode, String) {
    let mut req = Request::get(path);
    if let Some(id) = id {
        req = req.header(requests::REQUEST_ID_HEADER, id);
    }
    let res = router(state)
        .oneshot(req.body(Body::empty()).expect("request"))
        .await
        .expect("response");
    let echoed = res
        .headers()
        .get(requests::REQUEST_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    (res.status(), echoed)
}

#[tokio::test]
async fn every_response_carries_a_request_id_and_only_a_safe_one_is_repeated() {
    let state = dev(Provider::Mock(MockIncus::default()));
    assert_eq!(
        with_request_id(state.clone(), "/api/v1/health", Some("trace-42.a_b")).await,
        (StatusCode::OK, "trace-42.a_b".to_owned())
    );
    // Unknown routes and refusals get one too.
    let (status, id) = with_request_id(state.clone(), "/api/v1/nope", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(uuid::Uuid::parse_str(&id).is_ok(), "generated: {id}");
    for unsafe_id in ["has space", "a\"quote", &"x".repeat(65), ""] {
        let (_, id) = with_request_id(state.clone(), "/api/v1/health", Some(unsafe_id)).await;
        assert!(
            uuid::Uuid::parse_str(&id).is_ok(),
            "{unsafe_id:?} is replaced, got {id}"
        );
    }
    assert!(requests::acceptable(&"x".repeat(64)));
}

#[tokio::test]
async fn database_errors_name_the_next_step_without_the_database_text() {
    let refused = ApiError::from(apelora_persistence::StoreError::Database(
        sqlx::Error::Protocol("relation \"secret_table\" does not exist".into()),
    ));
    assert_eq!(
        (refused.status, refused.code),
        (StatusCode::INTERNAL_SERVER_ERROR, "store_error")
    );
    assert!(
        !refused.message.contains("secret_table") && refused.message.contains("request id"),
        "{}",
        refused.message
    );
    let down = ApiError::from(apelora_persistence::StoreError::Database(
        sqlx::Error::PoolTimedOut,
    ));
    assert_eq!(
        (down.status, down.code),
        (StatusCode::SERVICE_UNAVAILABLE, "store_unavailable")
    );
    assert!(down.message.contains("DATABASE_URL"), "{}", down.message);
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn an_unreachable_database_is_a_503(pool: Pool) {
    let state = AppState::new(
        Provider::Mock(MockIncus::default()),
        Auth::Tokens(pool.clone()),
    );
    pool.close().await;
    // Well-formed, so it is looked up in the database.
    let token = format!("apl_{}", "0".repeat(64));
    let (status, body, _) = call(state, "/api/v1/instances", Some(&token)).await;
    assert_eq!(
        (status, body["error"]["code"].as_str()),
        (StatusCode::SERVICE_UNAVAILABLE, Some("store_unavailable")),
        "{body}"
    );
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn tokens_are_required_and_roles_are_enforced(pool: Pool) {
    let state = AppState::new(
        Provider::Mock(MockIncus::default()),
        Auth::Tokens(pool.clone()),
    );
    let (status, body, www) = call(state.clone(), "/api/v1/instances", None).await;
    assert_eq!(
        (status, body["error"]["code"].as_str(), www.as_deref()),
        (
            StatusCode::UNAUTHORIZED,
            Some("unauthenticated"),
            Some("Bearer")
        )
    );
    let (status, _, _) = call(state.clone(), "/api/v1/instances", Some("apl_0000")).await;
    assert_eq!(status, 401);
    let (status, body, _) = call(state.clone(), "/api/v1/health", None).await;
    assert_eq!(
        (status, body["auth"].as_str()),
        (StatusCode::OK, Some("tokens")),
        "health stays public"
    );

    let (auditor, _) = apelora_persistence::create_token(&pool, "ci-auditor", "auditor", None)
        .await
        .expect("token");
    let (designer, designer_rec) =
        apelora_persistence::create_token(&pool, "alice", "studio-designer", None)
            .await
            .expect("token");
    apelora_persistence::audit(&pool, "cli", "token.create", "alice", &json!({}))
        .await
        .expect("audit");

    let (status, body, _) = call(state.clone(), "/api/v1/whoami", Some(&auditor)).await;
    assert_eq!(
        (status, body["role"].as_str()),
        (StatusCode::OK, Some("auditor"))
    );
    assert_eq!(
        call(state.clone(), "/api/v1/cluster", Some(&auditor))
            .await
            .0,
        200
    );
    let (status, body, _) = call(state.clone(), "/api/v1/audit", Some(&auditor)).await;
    assert_eq!(
        (status, body["items"][0]["action"].as_str()),
        (StatusCode::OK, Some("token.create"))
    );

    let (status, body, _) = call(state.clone(), "/api/v1/audit", Some(&designer)).await;
    assert_eq!(
        (status, body["error"]["code"].as_str()),
        (StatusCode::FORBIDDEN, Some("forbidden"))
    );
    assert_eq!(
        call(state.clone(), "/api/v1/instances", Some(&designer))
            .await
            .0,
        200
    );

    apelora_persistence::revoke_token(&pool, designer_rec.id)
        .await
        .expect("revoke");
    assert_eq!(
        call(state, "/api/v1/instances", Some(&designer)).await.0,
        401
    );
}

/// `packages/api-client/openapi.json` is the contract the TypeScript client is generated from.
/// Regenerate with `npm run api:generate` (or `UPDATE_OPENAPI=1 cargo test -p apelora-control-plane`).
#[test]
fn committed_openapi_document_is_current() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../packages/api-client/openapi.json"
    );
    let current = openapi_json();
    if std::env::var_os("UPDATE_OPENAPI").is_some() {
        std::fs::write(path, &current).expect("write openapi.json");
    }
    let committed = std::fs::read_to_string(path).unwrap_or_default();
    assert!(
        committed == current,
        "{path} is stale: run `npm run api:generate`"
    );
}

#[tokio::test]
async fn openapi_document_is_public_and_lists_every_route() {
    let (status, doc, _) = call(
        AppState::new(
            Provider::Mock(MockIncus::default()),
            Auth::DisabledForDevelopment,
        ),
        "/api/v1/openapi.json",
        None,
    )
    .await;
    assert_eq!(status, 200);
    for route in [
        "/api/v1/health",
        "/api/v1/whoami",
        "/api/v1/cluster",
        "/api/v1/instances",
        "/api/v1/projects",
        "/api/v1/networks",
        "/api/v1/storage-pools",
        "/api/v1/audit",
        "/api/v1/jobs/{id}",
        "/api/v1/preflight",
        "/api/v1/image-mirrors",
    ] {
        assert!(doc["paths"][route]["get"].is_object(), "{route} missing");
    }
    assert!(doc["paths"]["/api/v1/instances"]["post"].is_object());
    assert!(doc["paths"]["/api/v1/instances/{project}/{name}/state"]["post"].is_object());
    assert!(doc["paths"]["/api/v1/instances/{project}/{name}"]["delete"].is_object());
    assert!(doc["paths"]["/api/v1/cluster/join-tokens"]["post"].is_object());
    assert!(doc["paths"]["/api/v1/cluster/join-tokens"]["get"].is_object());
    assert!(doc["paths"]["/api/v1/cluster/join-tokens/{member}"]["delete"].is_object());
    assert!(doc["paths"]["/api/v1/cluster/members/{member}/evacuation"]["get"].is_object());
    assert!(doc["paths"]["/api/v1/cluster/members/{member}/evacuate"]["post"].is_object());
    assert!(doc["paths"]["/api/v1/cluster/members/{member}/restore"]["post"].is_object());
    assert!(doc["paths"]["/api/v1/tokens"]["post"].is_object());
    assert!(doc["paths"]["/api/v1/tokens/{id}"]["delete"].is_object());
    assert!(doc["paths"]["/api/v1/jobs"]["get"].is_object());
    assert!(doc["paths"]["/api/v1/system"]["get"].is_object());
    assert!(doc["paths"]["/api/v1/applications/{name}/deployments"]["post"].is_object());
    assert!(doc["paths"]["/api/v1/applications/{name}/plan"]["post"].is_object());
    assert_eq!(
        doc["paths"]["/api/v1/health"]["get"]["security"],
        json!([{}]),
        "health is public"
    );
    assert_eq!(
        doc["components"]["securitySchemes"]["bearer"]["scheme"],
        "bearer"
    );
}

// ---- Deployment requests ----------------------------------------------------------------

pub(super) async fn send(
    state: AppState,
    method: &str,
    path: &str,
    token: &str,
    body: Option<Value>,
    idempotency: Option<&str>,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    if let Some(k) = idempotency {
        req = req.header("idempotency-key", k);
    }
    let req = match body {
        Some(b) => req
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(b.to_string())),
        None => req.body(Body::empty()),
    }
    .expect("request");
    let res = router(state).oneshot(req).await.expect("response");
    let status = res.status();
    let bytes = res.into_body().collect().await.expect("body").to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn web_spec(project: &str, name: &str) -> Value {
    json!({ "project": project, "name": name, "kind": "container",
            "source": { "protocol": "oci", "server": "https://registry.example", "alias": "nginx:1.29" },
            "start": true })
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn deployment_requests_are_checked_queued_audited_and_carried_out(pool: Pool) {
    let provider = Provider::Mock(MockIncus::default());
    let state = AppState::new(provider.clone(), Auth::Tokens(pool.clone()));
    let token = |role: &'static str| {
        let pool = pool.clone();
        async move {
            apelora_persistence::create_token(&pool, role, role, None)
                .await
                .expect("token")
                .0
        }
    };
    let (operator, designer) = (token("operator").await, token("studio-designer").await);

    // Permission, project allow-list and validation are checked before anything is queued.
    let checks = [
        (&designer, web_spec("apelora", "web-01"), 403, "forbidden"),
        (
            &operator,
            web_spec("default", "web-01"),
            403,
            "project_not_allowed",
        ),
        (
            &operator,
            web_spec("apelora", "../x"),
            400,
            "invalid_request",
        ),
        (
            &operator,
            json!({ "name": "web-01" }),
            400,
            "invalid_request",
        ),
    ];
    for (tok, body, status, code) in checks {
        let (s, b) = send(
            state.clone(),
            "POST",
            "/api/v1/instances",
            tok,
            Some(body),
            None,
        )
        .await;
        assert_eq!(
            (s.as_u16(), b["error"]["code"].as_str()),
            (status, Some(code)),
            "{b}"
        );
    }
    let queued: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(queued, 0);

    // Accepted, idempotent per key.
    let (s, first) = send(
        state.clone(),
        "POST",
        "/api/v1/instances",
        &operator,
        Some(web_spec("apelora", "web-01")),
        Some("k1"),
    )
    .await;
    assert_eq!(
        (s, first["created"].as_bool()),
        (StatusCode::ACCEPTED, Some(true))
    );
    let (_, again) = send(
        state.clone(),
        "POST",
        "/api/v1/instances",
        &operator,
        Some(web_spec("apelora", "web-01")),
        Some("k1"),
    )
    .await;
    assert_eq!(
        (again["jobId"].clone(), again["created"].as_bool()),
        (first["jobId"].clone(), Some(false))
    );
    let job_path = format!("/api/v1/jobs/{}", first["jobId"].as_str().expect("id"));
    let (_, job) = send(state.clone(), "GET", &job_path, &designer, None, None).await;
    assert_eq!(
        (job["kind"].as_str(), job["status"].as_str()),
        (Some("instance.create"), Some("pending"))
    );

    // The worker carries it out against the same (mock) Incus.
    while apelora_deployment_worker::run_once(&pool, &provider, "w")
        .await
        .expect("run")
    {}
    let (_, job) = send(state.clone(), "GET", &job_path, &designer, None, None).await;
    assert_eq!(job["status"], "succeeded", "{job}");
    let (_, list) = send(
        state.clone(),
        "GET",
        "/api/v1/instances",
        &designer,
        None,
        None,
    )
    .await;
    let web = list["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|i| i["name"] == "web-01")
        .cloned();
    assert_eq!(
        web.map(|w| (w["project"].clone(), w["status"].clone())),
        Some((json!("apelora"), json!("Running")))
    );

    let (s, _) = send(
        state.clone(),
        "POST",
        "/api/v1/instances/apelora/web-01/state",
        &operator,
        Some(json!({ "action": "stop" })),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::ACCEPTED);
    let (s, _) = send(
        state.clone(),
        "POST",
        "/api/v1/instances/apelora/web-01/state",
        &operator,
        Some(json!({ "action": "reboot" })),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = send(
        state.clone(),
        "DELETE",
        "/api/v1/instances/apelora/web-01",
        &operator,
        None,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::ACCEPTED);
    while apelora_deployment_worker::run_once(&pool, &provider, "w")
        .await
        .expect("run")
    {}
    assert_eq!(provider.instance("apelora", "web-01").await, Ok(None));

    let actions: Vec<String> = sqlx::query_scalar(
        "SELECT action FROM audit_events WHERE action <> 'access.denied' ORDER BY id",
    )
    .fetch_all(&pool)
    .await
    .expect("audit");
    assert_eq!(
        actions,
        [
            "instance.create.requested",
            "instance.stop.requested",
            "instance.delete.requested"
        ]
    );

    let (s, b) = send(
        state.clone(),
        "GET",
        &format!("/api/v1/jobs/{}", uuid::Uuid::new_v4()),
        &designer,
        None,
        None,
    )
    .await;
    assert_eq!(
        (s, b["error"]["code"].as_str()),
        (StatusCode::NOT_FOUND, Some("not_found"))
    );
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn one_request_id_links_the_request_its_audit_row_the_job_and_the_incus_operation(
    pool: Pool,
) {
    let provider = Provider::Mock(MockIncus::default());
    let state = AppState::new(provider.clone(), Auth::Tokens(pool.clone()));
    let (operator, _) = apelora_persistence::create_token(&pool, "operator", "operator", None)
        .await
        .expect("token");
    let req = Request::post("/api/v1/instances")
        .header(header::AUTHORIZATION, format!("Bearer {operator}"))
        .header(header::CONTENT_TYPE, "application/json")
        .header(requests::REQUEST_ID_HEADER, "trace-7.create")
        .body(Body::from(web_spec("apelora", "web-01").to_string()))
        .expect("request");
    let res = router(state.clone()).oneshot(req).await.expect("response");
    assert_eq!(res.status(), StatusCode::ACCEPTED);
    let bytes = res.into_body().collect().await.expect("body").to_bytes();
    let accepted: Value = serde_json::from_slice(&bytes).expect("json");
    let job_id = accepted["jobId"].as_str().expect("job id").to_owned();

    // The audit row of the request names the job and the request.
    let (detail_job, detail_request): (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT detail->>'jobId', detail->>'requestId' FROM audit_events
         WHERE action = 'instance.create.requested'",
    )
    .fetch_one(&pool)
    .await
    .expect("audit row");
    assert_eq!(
        (detail_job.as_deref(), detail_request.as_deref()),
        (Some(job_id.as_str()), Some("trace-7.create"))
    );

    // The job carries the request id; once run, the Incus operation it started is kept.
    while apelora_deployment_worker::run_once(&pool, &provider, "w")
        .await
        .expect("run")
    {}
    let (_, job) = send(
        state.clone(),
        "GET",
        &format!("/api/v1/jobs/{job_id}"),
        &operator,
        None,
        None,
    )
    .await;
    assert_eq!(job["status"], "succeeded", "{job}");
    assert_eq!(job["requestId"], "trace-7.create");
    assert_eq!(
        job["incusOperation"],
        Value::Null,
        "nothing is waited on now"
    );
    assert_eq!(
        job["incusOperations"].as_array().map(Vec::len),
        Some(1),
        "{job}"
    );
    let (_, list) = send(state, "GET", "/api/v1/jobs", &operator, None, None).await;
    assert_eq!(list["items"][0]["requestId"], "trace-7.create");
}

#[tokio::test]
async fn deployments_need_a_database() {
    let state = AppState::new(
        Provider::Mock(MockIncus::default()),
        Auth::DisabledForDevelopment,
    );
    let (s, b) = send(
        state,
        "POST",
        "/api/v1/instances",
        "x",
        Some(web_spec("apelora", "web-01")),
        None,
    )
    .await;
    assert_eq!(
        (s, b["error"]["code"].as_str()),
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Some("database_not_configured")
        )
    );
}

#[tokio::test]
async fn preflight_reports_checks_without_claiming_unmeasured_ones() {
    let (status, body, _) = call(
        dev(Provider::Mock(MockIncus::with_extensions(&[
            "instance_create_start",
            "operation_wait",
        ]))),
        "/api/v1/preflight",
        None,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body["source"]["provider"], "mock");
    let status_of = |id: &str| {
        body["checks"]
            .as_array()
            .and_then(|c| c.iter().find(|c| c["id"] == id))
            .map(|c| c["status"].clone())
    };
    assert_eq!(status_of("api-extensions"), Some(json!("pass")));
    assert_eq!(status_of("oci"), Some(json!("warn")));
    assert_eq!(status_of("project-apelora"), Some(json!("pass")));
    for id in ["incus-version", "kernel-version", "clock", "latency"] {
        assert_eq!(status_of(id), Some(json!("not-checked")), "{id}");
    }
    assert_eq!(body["overall"], "warn");
    // No mirrors configured: one informational warning, nothing probed.
    assert_eq!(status_of("image-mirrors"), Some(json!("warn")));
}

#[tokio::test]
async fn image_mirrors_show_the_configuration_and_coverage_per_catalog_server() {
    let mirrors = apelora_domain::images::ImageMirrors::parse(
        "https://images.linuxcontainers.org=https://127.0.0.1:1",
        true,
    )
    .expect("valid");
    let state = dev(Provider::Mock(MockIncus::default())).with_image_mirrors(mirrors);
    let (status, body, _) = call(state.clone(), "/api/v1/image-mirrors", None).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["mode"], "mirrors-only");
    assert_eq!(
        body["mirrors"][0]["upstream"],
        "https://images.linuxcontainers.org"
    );
    assert_eq!(body["mirrors"][0]["mirror"], "https://127.0.0.1:1");
    assert_eq!(body["mirrors"][0]["probe"]["state"], "fails");
    assert!(body["mirrors"][0]["checkedAt"].is_string());
    let server = |s: &str| {
        body["servers"]
            .as_array()
            .and_then(|a| a.iter().find(|x| x["server"] == s))
            .cloned()
            .unwrap_or_default()
    };
    assert_eq!(
        server("https://images.linuxcontainers.org")["coverage"],
        "mirrored"
    );
    assert_eq!(server("https://docker.io")["coverage"], "refused");
    assert_eq!(server("https://docker.io")["protocol"], "oci");
    assert_eq!(server("https://docker.io")["images"][0]["class"], "nginx");
    assert_eq!(body["overall"], "fail");
    // Preflight shows the same checks.
    let (_, pre, _) = call(state, "/api/v1/preflight", None).await;
    let ids: Vec<&str> = pre["checks"]
        .as_array()
        .map(|c| c.iter().filter_map(|c| c["id"].as_str()).collect())
        .unwrap_or_default();
    for id in [
        "image-mirrors",
        "image-server-images.linuxcontainers.org",
        "image-server-docker.io",
    ] {
        assert!(ids.contains(&id), "{id} missing from {ids:?}");
    }
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn image_mirrors_need_read_inventory(pool: Pool) {
    let state = AppState::new(
        Provider::Mock(MockIncus::default()),
        Auth::Tokens(pool.clone()),
    );
    let (status, _, _) = call(state.clone(), "/api/v1/image-mirrors", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let auditor = apelora_persistence::create_token(&pool, "aud", "auditor", None)
        .await
        .expect("token")
        .0;
    let (status, body, _) = call(state, "/api/v1/image-mirrors", Some(&auditor)).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["mode"], "any");
}

// ---- Cluster membership --------------------------------------------------------------------

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn join_tokens_are_admin_only_shown_once_audited_without_the_secret(pool: Pool) {
    let mock = Provider::Mock(MockIncus::default());
    let state = AppState::new(mock.clone(), Auth::Tokens(pool.clone())).with_admin(Some(mock));
    let admin = apelora_persistence::create_token(&pool, "admin", "fabric-admin", None)
        .await
        .expect("token")
        .0;
    let operator = apelora_persistence::create_token(&pool, "op", "operator", None)
        .await
        .expect("token")
        .0;
    let path = "/api/v1/cluster/join-tokens";
    let body = json!({ "memberName": "mock-member-04" });

    let (status, err) = send(
        state.clone(),
        "POST",
        path,
        &operator,
        Some(body.clone()),
        None,
    )
    .await;
    assert_eq!(
        (status, err["error"]["code"].as_str()),
        (StatusCode::FORBIDDEN, Some("forbidden"))
    );
    let (status, _) = send(state.clone(), "GET", path, &operator, None, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, err) = send(
        state.clone(),
        "POST",
        path,
        &admin,
        Some(json!({ "memberName": "../x" })),
        None,
    )
    .await;
    assert_eq!(
        (status, err["error"]["code"].as_str()),
        (StatusCode::BAD_REQUEST, Some("invalid_request"))
    );

    let res = router(state.clone())
        .oneshot(
            Request::post(path)
                .header(header::AUTHORIZATION, format!("Bearer {admin}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(res.status(), StatusCode::CREATED);
    assert_eq!(res.headers()[header::CACHE_CONTROL], "no-store");
    let created: Value =
        serde_json::from_slice(&res.into_body().collect().await.expect("body").to_bytes())
            .expect("json");
    assert_eq!(created["memberName"], "mock-member-04");
    let secret = created["token"].as_str().expect("token").to_owned();

    let (status, list) = send(state.clone(), "GET", path, &admin, None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["source"]["provider"], "mock");
    assert_eq!(list["items"][0]["memberName"], "mock-member-04");
    assert!(
        !list.to_string().contains(&secret),
        "the list never carries the secret"
    );

    let one = format!("{path}/mock-member-04");
    assert_eq!(
        send(state.clone(), "DELETE", &one, &admin, None, None)
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    let (status, err) = send(state.clone(), "DELETE", &one, &admin, None, None).await;
    assert_eq!(
        (status, err["error"]["code"].as_str()),
        (StatusCode::NOT_FOUND, Some("not_found"))
    );

    let audit: Vec<(String, String, String)> =
        sqlx::query_as("SELECT action, target, detail::text FROM audit_events WHERE action <> 'access.denied' ORDER BY id")
            .fetch_all(&pool)
            .await
            .expect("audit");
    let actions: Vec<&str> = audit.iter().map(|(a, _, _)| a.as_str()).collect();
    assert_eq!(
        actions,
        ["cluster.join-token.created", "cluster.join-token.revoked"]
    );
    assert!(
        audit
            .iter()
            .all(|(_, t, d)| t == "mock-member-04" && !d.contains(&secret))
    );
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn join_tokens_need_the_admin_identity(pool: Pool) {
    let state = AppState::new(
        Provider::Mock(MockIncus::default()),
        Auth::Tokens(pool.clone()),
    );
    let admin = apelora_persistence::create_token(&pool, "admin", "fabric-admin", None)
        .await
        .expect("token")
        .0;
    let (status, err) = send(
        state,
        "POST",
        "/api/v1/cluster/join-tokens",
        &admin,
        Some(json!({ "memberName": "m4" })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(err["error"]["code"], "cluster_admin_not_configured");
}

#[test]
fn long_lived_join_tokens_get_a_warning() {
    let now = time::OffsetDateTime::parse(
        "2026-10-20T00:00:00Z",
        &time::format_description::well_known::Rfc3339,
    )
    .expect("time");
    assert!(cluster_admin::lifetime_warnings("2026-10-20T00:30:00Z", now).is_empty());
    let w = cluster_admin::lifetime_warnings("2026-10-20T03:00:00.123456789+00:00", now);
    assert!(w[0].contains("cluster.join_token_expiry"), "{w:?}");
    assert!(cluster_admin::lifetime_warnings("mock: never used", now).is_empty());
}

// ---- Administration ------------------------------------------------------------------------

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn api_tokens_are_created_once_listed_without_secrets_and_revoked(pool: Pool) {
    let state = AppState::new(
        Provider::Mock(MockIncus::default()),
        Auth::Tokens(pool.clone()),
    );
    let admin = apelora_persistence::create_token(&pool, "admin", "fabric-admin", None)
        .await
        .expect("token")
        .0;
    let operator = apelora_persistence::create_token(&pool, "op", "operator", None)
        .await
        .expect("token")
        .0;

    let (status, _) = send(
        state.clone(),
        "GET",
        "/api/v1/tokens",
        &operator,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    for (body, why) in [
        (json!({ "name": "", "role": "auditor" }), "empty name"),
        (json!({ "name": "ci", "role": "root" }), "unknown role"),
        (
            json!({ "name": "ci", "role": "auditor", "expiresInDays": 0 }),
            "no lifetime",
        ),
        (
            json!({ "name": "ci", "role": "auditor", "expiresInDays": 366 }),
            "over a year",
        ),
    ] {
        let (status, _) = send(
            state.clone(),
            "POST",
            "/api/v1/tokens",
            &admin,
            Some(body),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{why}");
    }

    let res = router(state.clone())
        .oneshot(
            Request::post("/api/v1/tokens")
                .header(header::AUTHORIZATION, format!("Bearer {admin}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({ "name": "ci", "role": "auditor", "expiresInDays": 7 }).to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(res.status(), StatusCode::CREATED);
    assert_eq!(res.headers()[header::CACHE_CONTROL], "no-store");
    let created: Value =
        serde_json::from_slice(&res.into_body().collect().await.expect("body").to_bytes())
            .expect("json");
    let secret = created["token"].as_str().expect("token").to_owned();
    assert_eq!(created["info"]["role"], "auditor");
    assert!(
        created["info"]["expiresAt"].is_string(),
        "API tokens always expire"
    );

    let (status, me) = send(state.clone(), "GET", "/api/v1/whoami", &secret, None, None).await;
    assert_eq!(
        (status, me["role"].as_str()),
        (StatusCode::OK, Some("auditor"))
    );
    let (_, list) = send(state.clone(), "GET", "/api/v1/tokens", &admin, None, None).await;
    assert_eq!(list["items"].as_array().map(Vec::len), Some(3));
    assert!(!list.to_string().contains(&secret));

    let id = created["info"]["id"].as_str().expect("id").to_owned();
    let path = format!("/api/v1/tokens/{id}");
    assert_eq!(
        send(state.clone(), "DELETE", &path, &admin, None, None)
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        send(state.clone(), "DELETE", &path, &admin, None, None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let (status, _) = send(state.clone(), "GET", "/api/v1/whoami", &secret, None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "revoked at once");

    let audit: Vec<(String, String)> = sqlx::query_as(
        "SELECT action, detail::text FROM audit_events WHERE action <> 'access.denied' ORDER BY id",
    )
    .fetch_all(&pool)
    .await
    .expect("audit");
    assert_eq!(
        audit.iter().map(|(a, _)| a.as_str()).collect::<Vec<_>>(),
        ["token.create", "token.revoke"]
    );
    assert!(audit.iter().all(|(_, d)| !d.contains(&secret)));
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn recent_jobs_and_system_information_are_listed(pool: Pool) {
    let state = AppState::new(
        Provider::Mock(MockIncus::default()),
        Auth::Tokens(pool.clone()),
    );
    let auditor = apelora_persistence::create_token(&pool, "audit", "auditor", None)
        .await
        .expect("token")
        .0;
    apelora_persistence::enqueue_job(
        &pool,
        "instance.stop",
        None,
        &json!({ "project": "apelora", "name": "web-01" }),
        None,
    )
    .await
    .expect("enqueue");
    let (status, jobs) = send(state.clone(), "GET", "/api/v1/jobs", &auditor, None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(jobs["items"][0]["kind"], "instance.stop");
    assert_eq!(jobs["items"][0]["request"]["name"], "web-01");
    let (status, system) = send(state, "GET", "/api/v1/system", &auditor, None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(system["incusSeries"], "7.0");
    assert!(
        system["incusPinned"]
            .as_str()
            .is_some_and(|v| v.starts_with("v7.0."))
    );
}

// ---- Applications ----------------------------------------------------------------------------

pub(super) fn module(id: &str) -> Value {
    json!({ "module": id, "spec": {
        "project": "ignored", "name": "ignored", "kind": "container", "start": true,
        "source": { "protocol": "simplestreams", "server": "https://images.example", "alias": "debian/13" } } })
}

pub(super) async fn drain(pool: &Pool, provider: &Provider) {
    while apelora_deployment_worker::run_once(pool, provider, "w")
        .await
        .expect("worker")
    {}
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn applications_are_planned_deployed_revised_and_reported_by_module(pool: Pool) {
    let provider = Provider::Mock(MockIncus::default());
    let state = AppState::new(provider.clone(), Auth::Tokens(pool.clone()))
        .with_deploy_projects(vec!["apelora".into(), "default".into()]);
    let operator = apelora_persistence::create_token(&pool, "op", "operator", None)
        .await
        .expect("token")
        .0;
    let designer = apelora_persistence::create_token(&pool, "designer", "studio-designer", None)
        .await
        .expect("token")
        .0;
    let doc = json!({ "apiVersion": "fabric.apelora/v1alpha1", "kind": "ApplicationAssembly" });
    let both = json!({ "project": "apelora", "modules": [module("web-01"), module("db-01")] });

    // Only `deploy` may plan; names are checked before Incus is asked.
    assert_eq!(
        send(
            state.clone(),
            "POST",
            "/api/v1/applications/shop/plan",
            &designer,
            Some(both.clone()),
            None
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(
            state.clone(),
            "POST",
            "/api/v1/applications/Shop_1/plan",
            &operator,
            Some(both.clone()),
            None
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let twice = json!({ "project": "apelora", "modules": [module("web-01"), module("web-01")] });
    assert_eq!(
        send(
            state.clone(),
            "POST",
            "/api/v1/applications/shop/plan",
            &operator,
            Some(twice),
            None
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );

    let (status, plan) = send(
        state.clone(),
        "POST",
        "/api/v1/applications/shop/plan",
        &operator,
        Some(both.clone()),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(plan["items"][0]["instance"], "shop-web-01");
    assert_eq!(plan["items"][0]["action"], "create");
    assert!(
        plan["warnings"][0]
            .as_str()
            .is_some_and(|w| w.contains("single points of failure: db-01, web-01"))
    );

    let mut body = both.clone();
    body["document"] = doc.clone();
    let (status, first) = send(
        state.clone(),
        "POST",
        "/api/v1/applications/shop/deployments",
        &operator,
        Some(body.clone()),
        Some("d1"),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{first}");
    assert_eq!(first["revision"], 1);
    assert_eq!(first["jobs"].as_array().map(Vec::len), Some(2));
    let (_, again) = send(
        state.clone(),
        "POST",
        "/api/v1/applications/shop/deployments",
        &operator,
        Some(body.clone()),
        Some("d1"),
    )
    .await;
    assert_eq!(
        again["jobs"][0]["jobId"], first["jobs"][0]["jobId"],
        "the same key queues nothing new"
    );
    assert_eq!(again["revision"], 1, "and saves no new revision");
    drain(&pool, &provider).await;

    let (_, st) = send(
        state.clone(),
        "GET",
        "/api/v1/applications/shop/status?project=apelora",
        &operator,
        None,
        None,
    )
    .await;
    let mut seen: Vec<(String, String)> = st["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|i| {
            (
                i["module"].as_str().unwrap_or_default().to_owned(),
                i["status"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect();
    seen.sort();
    assert_eq!(
        seen,
        vec![
            ("db-01".into(), "Running".into()),
            ("web-01".into(), "Running".into())
        ]
    );

    // Drop db-01 from the design: kept unless removal is asked for.
    let mut one = json!({ "project": "apelora", "modules": [module("web-01")], "document": doc });
    let (_, keep) = send(
        state.clone(),
        "POST",
        "/api/v1/applications/shop/deployments",
        &operator,
        Some(one.clone()),
        None,
    )
    .await;
    assert_eq!(keep["revision"], 2);
    assert_eq!(keep["jobs"].as_array().map(Vec::len), Some(0));
    assert_eq!(keep["plan"]["items"][1]["action"], "remove");
    one["removeMissing"] = json!(true);
    let (_, removed) = send(
        state.clone(),
        "POST",
        "/api/v1/applications/shop/deployments",
        &operator,
        Some(one),
        None,
    )
    .await;
    assert_eq!(removed["jobs"][0]["instance"], "shop-db-01");
    drain(&pool, &provider).await;
    let (_, st) = send(
        state.clone(),
        "GET",
        "/api/v1/applications/shop/status",
        &operator,
        None,
        None,
    )
    .await;
    assert_eq!(st["items"].as_array().map(Vec::len), Some(1));

    // The seeded mock-web-01 in "default" is not Apelora's: deploying over it is refused.
    let clash = json!({ "project": "default", "modules": [module("web-01")], "document": {} });
    let (status, err) = send(
        state.clone(),
        "POST",
        "/api/v1/applications/mock/deployments",
        &operator,
        Some(clash),
        None,
    )
    .await;
    assert_eq!(
        (status, err["error"]["code"].as_str()),
        (StatusCode::CONFLICT, Some("plan_conflict"))
    );
    assert!(
        err["error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("not created by Apelora"))
    );

    let (_, apps) = send(
        state.clone(),
        "GET",
        "/api/v1/applications",
        &operator,
        None,
        None,
    )
    .await;
    assert_eq!(
        apps["items"].as_array().map(Vec::len),
        Some(1),
        "a refused deployment saves nothing"
    );
    assert_eq!(apps["items"][0]["latestRevision"], 3);
    let audit: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE action = 'application.deploy.requested'",
    )
    .fetch_one(&pool)
    .await
    .expect("audit");
    assert_eq!(audit, 3, "the repeated request is not audited twice");
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn connections_become_an_application_network_with_acls_on_both_sides(pool: Pool) {
    let mock = MockIncus::default();
    let provider = Provider::Mock(mock.clone());
    let state = AppState::new(provider.clone(), Auth::Tokens(pool.clone()));
    let operator = apelora_persistence::create_token(&pool, "op", "operator", None)
        .await
        .expect("token")
        .0;
    let conn = json!([{ "from": "web-01", "to": "db-01", "transport": "tcp", "port": 5432 }]);
    let body = |connections: Value| json!({ "project": "apelora", "document": {}, "modules": [module("web-01"), module("db-01"), module("cache-01")], "connections": connections });

    // Without an uplink nothing is enforced, and the plan says so.
    let (_, plan) = send(
        state.clone(),
        "POST",
        "/api/v1/applications/shop/plan",
        &operator,
        Some(body(conn.clone())),
        None,
    )
    .await;
    assert!(
        plan["warnings"]
            .as_array()
            .expect("warnings")
            .iter()
            .any(|w| w.as_str().is_some_and(|w| w.contains("not enforced")))
    );
    let bad = json!([{ "from": "web-01", "to": "nope-01", "transport": "tcp", "port": 1 }]);
    assert_eq!(
        send(
            state.clone(),
            "POST",
            "/api/v1/applications/shop/plan",
            &operator,
            Some(body(bad)),
            None
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );

    let state = state.with_network_uplink(Some("incusbr0".into()));
    let (status, d) = send(
        state.clone(),
        "POST",
        "/api/v1/applications/shop/deployments",
        &operator,
        Some(body(conn)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{d}");
    assert!(d["networkJob"].is_string());
    let untraced: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE request_id IS NULL")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(untraced, 0, "every job of the deployment names its request");
    let create: Value = sqlx::query_scalar(
        "SELECT payload FROM jobs WHERE kind = 'instance.create' ORDER BY created_at LIMIT 1",
    )
    .fetch_one(&pool)
    .await
    .expect("job");
    assert_eq!(
        create["network"],
        json!({ "network": "shop", "acl": "shop-web-01", "uplink": "incusbr0" })
    );
    drain(&pool, &provider).await;
    let job: String = sqlx::query_scalar("SELECT status FROM jobs WHERE kind = 'network.sync'")
        .fetch_one(&pool)
        .await
        .expect("sync");
    assert_eq!(job, "succeeded");

    let net = mock.net().clone();
    assert!(net.networks.contains("apelora/shop"));
    let (web_in, web_out) = &net.acls["apelora/shop-web-01"];
    assert!(web_in.is_empty());
    assert_eq!(
        (
            web_out[0].destination.as_str(),
            web_out[0].destination_port.as_str()
        ),
        ("$shop-db-01", "5432")
    );
    let (db_in, db_out) = &net.acls["apelora/shop-db-01"];
    assert_eq!(db_in[0].source, "$shop-web-01");
    assert!(db_out.is_empty());
    let (cache_in, cache_out) = &net.acls["apelora/shop-cache-01"];
    assert!(
        cache_in.is_empty() && cache_out.is_empty(),
        "unconnected modules are isolated"
    );
    assert!(net.address_sets["apelora/shop-web-01"][0].starts_with("198.51.100."));
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn removed_modules_lose_their_network_objects_and_other_applications_keep_theirs(pool: Pool) {
    let mock = MockIncus::default();
    let provider = Provider::Mock(mock.clone());
    let state = AppState::new(provider.clone(), Auth::Tokens(pool.clone()))
        .with_network_uplink(Some("incusbr0".into()));
    let operator = apelora_persistence::create_token(&pool, "op", "operator", None)
        .await
        .expect("token")
        .0;
    let conn = json!([{ "from": "web-01", "to": "db-01", "transport": "tcp", "port": 5432 }]);
    let deploy = |app: &'static str, modules: Value, connections: Value| {
        let state = state.clone();
        let operator = operator.clone();
        async move {
            let body = json!({ "project": "apelora", "document": {}, "modules": modules, "connections": connections, "removeMissing": true });
            let (status, d) = send(
                state,
                "POST",
                &format!("/api/v1/applications/{app}/deployments"),
                &operator,
                Some(body),
                None,
            )
            .await;
            assert_eq!(status, StatusCode::ACCEPTED, "{d}");
        }
    };
    deploy(
        "shop",
        json!([module("web-01"), module("db-01")]),
        conn.clone(),
    )
    .await;
    // "shop-eu" starts with "shop-": a name prefix must never decide ownership.
    deploy("shop-eu", json!([module("web-01"), module("db-01")]), conn).await;
    drain(&pool, &provider).await;
    let eu_before = mock
        .net()
        .acls
        .keys()
        .filter(|k| k.contains("shop-eu"))
        .count();
    assert_eq!(eu_before, 2);

    deploy("shop", json!([module("web-01")]), json!([])).await;
    drain(&pool, &provider).await;
    let net = mock.net().clone();
    assert!(!net.acls.contains_key("apelora/shop-db-01"));
    assert!(!net.address_sets.contains_key("apelora/shop-db-01"));
    assert!(net.acls.contains_key("apelora/shop-web-01"));
    assert_eq!(
        net.acls.keys().filter(|k| k.contains("shop-eu")).count(),
        2,
        "the other application is untouched"
    );

    deploy("shop", json!([]), json!([])).await;
    drain(&pool, &provider).await;
    let net = mock.net().clone();
    assert!(
        !net.networks.contains("apelora/shop"),
        "the network goes with the last module"
    );
    assert!(net.networks.contains("apelora/shop-eu"));
    assert!(net.address_sets.keys().all(|k| k.contains("shop-eu")));
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn stateless_replicas_are_deployed_recorded_and_restored(pool: Pool) {
    let provider = Provider::Mock(MockIncus::default());
    let state = AppState::new(provider.clone(), Auth::Tokens(pool.clone()));
    let operator = apelora_persistence::create_token(&pool, "op", "operator", None)
        .await
        .expect("token")
        .0;
    let replicated = |availability: &str| {
        let mut web = module("web-01");
        web["replicas"] = json!(2);
        web["availability"] = json!(availability);
        json!({ "project": "apelora", "document": {}, "modules": [web, module("db-01")] })
    };

    // Only stateless modules may have more than one replica.
    let (status, body) = send(
        state.clone(),
        "POST",
        "/api/v1/applications/shop/plan",
        &operator,
        Some(replicated("stateful")),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    let (status, plan) = send(
        state.clone(),
        "POST",
        "/api/v1/applications/shop/plan",
        &operator,
        Some(replicated("stateless")),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let names: Vec<_> = plan["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|i| {
            (
                i["instance"].as_str().unwrap_or_default().to_owned(),
                i["replica"].clone(),
            )
        })
        .collect();
    assert_eq!(
        names,
        vec![
            ("shop-web-01".to_owned(), json!(1)),
            ("shop-web-01-r2".to_owned(), json!(2)),
            ("shop-db-01".to_owned(), json!(1)),
        ],
        "in the design's order"
    );
    assert!(
        plan["warnings"]
            .as_array()
            .expect("warnings")
            .iter()
            .any(|w| w.as_str() == Some("single points of failure: db-01 runs as one instance")),
        "{plan}"
    );

    let (status, _) = send(
        state.clone(),
        "POST",
        "/api/v1/applications/shop/deployments",
        &operator,
        Some(replicated("stateless")),
        Some("replicas-1"),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    drain(&pool, &provider).await;
    let records = apelora_persistence::list_deployments(&pool)
        .await
        .expect("records");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].desired[0]["replicas"], json!(2));

    // A lost replica comes back through the supervisor, labelled like the original.
    provider
        .change_state(
            "apelora",
            "shop-web-01-r2",
            apelora_domain::InstanceAction::Stop,
            true,
        )
        .await
        .expect("stop it");
    provider
        .delete_instance("apelora", "shop-web-01-r2")
        .await
        .expect("lose it");
    assert_eq!(
        apelora_deployment_worker::supervisor::supervise_once(&pool, &provider)
            .await
            .expect("supervise"),
        vec!["shop-web-01-r2".to_owned()]
    );
    drain(&pool, &provider).await;
    let (_, status) = send(
        state,
        "GET",
        "/api/v1/applications/shop/status",
        &operator,
        None,
        None,
    )
    .await;
    let mut instances: Vec<_> = status["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|i| i["instance"].as_str().unwrap_or_default().to_owned())
        .collect();
    instances.sort();
    assert_eq!(instances, ["shop-db-01", "shop-web-01", "shop-web-01-r2"]);
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn connected_stateless_modules_get_a_service_address_until_removed(pool: Pool) {
    let mock = MockIncus::default();
    let provider = Provider::Mock(mock.clone());
    let state = AppState::new(provider.clone(), Auth::Tokens(pool.clone()))
        .with_network_uplink(Some("incusbr0".into()))
        .with_service_range(Some("198.18.100.0/29".into()));
    let operator = apelora_persistence::create_token(&pool, "op", "operator", None)
        .await
        .expect("token")
        .0;
    let mut web = module("web-01");
    web["replicas"] = json!(2);
    web["availability"] = json!("stateless");
    let deploy = |modules: Value, connections: Value, key: &'static str| {
        let state = state.clone();
        let operator = operator.clone();
        async move {
            let (status, body) = send(
                state,
                "POST",
                "/api/v1/applications/shop/deployments",
                &operator,
                Some(
                    json!({ "project": "apelora", "document": {}, "modules": modules,
                             "connections": connections, "removeMissing": true }),
                ),
                Some(key),
            )
            .await;
            assert_eq!(status, StatusCode::ACCEPTED, "{body}");
        }
    };
    deploy(
        json!([module("client-01"), web.clone(), module("db-01")]),
        json!([{ "from": "client-01", "to": "web-01", "transport": "tcp", "port": 80 },
               { "from": "web-01", "to": "db-01", "transport": "tcp", "port": 5432 }]),
        "svc-1",
    )
    .await;
    drain(&pool, &provider).await;

    // Only web-01 is stateless and connected to: one load balancer over both replicas.
    let lbs = mock.net().load_balancers.clone();
    assert_eq!(
        lbs.keys().collect::<Vec<_>>(),
        ["apelora/shop/198.18.100.1"]
    );
    let (owner, lb) = &lbs["apelora/shop/198.18.100.1"];
    assert_eq!((owner.as_str(), lb.module.as_str()), ("shop", "web-01"));
    assert_eq!(lb.ports, vec![("tcp".to_owned(), 80)]);
    assert!(
        mock.net().health_check_repairs >= 1,
        "a new load balancer gets its health check re-applied"
    );
    assert_eq!(
        lb.backends
            .iter()
            .map(|b| b.name.as_str())
            .collect::<Vec<_>>(),
        ["r1", "r2"]
    );
    assert!(
        lb.backends[1].description.starts_with("shop-web-01-r2 "),
        "{:?}",
        lb.backends[1]
    );
    let (_, status) = send(
        state.clone(),
        "GET",
        "/api/v1/applications/shop/status",
        &operator,
        None,
        None,
    )
    .await;
    assert_eq!(
        status["services"],
        json!([{ "module": "web-01", "project": "apelora", "address": "198.18.100.1" }])
    );

    // web-01 leaves the design: its load balancer goes and its address is free again.
    deploy(
        json!([module("client-01"), module("db-01")]),
        json!([{ "from": "client-01", "to": "db-01", "transport": "tcp", "port": 5432 }]),
        "svc-2",
    )
    .await;
    drain(&pool, &provider).await;
    assert!(mock.net().load_balancers.is_empty());
    let (_, status) = send(
        state,
        "GET",
        "/api/v1/applications/shop/status",
        &operator,
        None,
        None,
    )
    .await;
    assert_eq!(status["services"], json!([]));
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn evacuation_is_previewed_then_confirmed_audited_and_restored(pool: Pool) {
    let mock = Provider::Mock(MockIncus::default());
    let state =
        AppState::new(mock.clone(), Auth::Tokens(pool.clone())).with_admin(Some(mock.clone()));
    let admin = apelora_persistence::create_token(&pool, "admin", "fabric-admin", None)
        .await
        .expect("token")
        .0;
    let operator = apelora_persistence::create_token(&pool, "op", "operator", None)
        .await
        .expect("token")
        .0;
    let base = "/api/v1/cluster/members/mock-member-02";

    // Anyone who reads the inventory may preview; it changes nothing.
    let (status, plan) = send(
        state.clone(),
        "GET",
        &format!("{base}/evacuation"),
        &operator,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{plan}");
    assert_eq!(plan["plan"]["member"], "mock-member-02");
    assert_eq!(
        plan["plan"]["targets"],
        json!(["mock-member-01", "mock-member-03"])
    );
    assert_eq!(plan["plan"]["blocked"], json!([]));
    let items = plan["plan"]["items"].as_array().expect("items");
    assert!(
        items
            .iter()
            .any(|i| i["instance"] == "mock-web-02" && i["impact"] == "not Apelora's")
    );
    let (status, _) = send(
        state.clone(),
        "GET",
        "/api/v1/cluster/members/nope/evacuation",
        &operator,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Evacuating needs manage-infrastructure and the member's name typed again.
    let confirm = |name: &str| Some(json!({ "confirm": name }));
    let (status, _) = send(
        state.clone(),
        "POST",
        &format!("{base}/evacuate"),
        &operator,
        confirm("mock-member-02"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, err) = send(
        state.clone(),
        "POST",
        &format!("{base}/evacuate"),
        &admin,
        confirm("mock-member-03"),
        None,
    )
    .await;
    assert_eq!(
        (status, err["error"]["code"].as_str()),
        (StatusCode::BAD_REQUEST, Some("confirmation_mismatch"))
    );
    let (status, started) = send(
        state.clone(),
        "POST",
        &format!("{base}/evacuate"),
        &admin,
        confirm("mock-member-02"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{started}");
    assert!(started["operation"].as_str().is_some_and(|o| !o.is_empty()));

    // An evacuated member cannot be evacuated again; restoring brings it back.
    let (status, err) = send(
        state.clone(),
        "POST",
        &format!("{base}/evacuate"),
        &admin,
        confirm("mock-member-02"),
        None,
    )
    .await;
    assert_eq!(
        (status, err["error"]["code"].as_str()),
        (StatusCode::CONFLICT, Some("evacuation_blocked"))
    );
    let (status, _) = send(
        state.clone(),
        "POST",
        &format!("{base}/restore"),
        &admin,
        confirm("mock-member-02"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let actions: Vec<String> = apelora_persistence::recent_audit(&pool, 10)
        .await
        .expect("audit")
        .into_iter()
        .filter(|e| e.target == "mock-member-02")
        .map(|e| e.action)
        .collect();
    assert_eq!(
        actions,
        ["cluster.member.restore", "cluster.member.evacuate"]
    );

    // Without the admin identity nothing can be evacuated, and the preview says what it cannot see.
    let restricted = AppState::new(mock, Auth::Tokens(pool.clone()));
    let (status, _) = send(
        restricted.clone(),
        "POST",
        &format!("{base}/evacuate"),
        &admin,
        confirm("mock-member-02"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let (_, plan) = send(
        restricted,
        "GET",
        &format!("{base}/evacuation"),
        &admin,
        None,
        None,
    )
    .await;
    assert!(
        plan["plan"]["warnings"]
            .as_array()
            .expect("warnings")
            .iter()
            .any(|w| w
                .as_str()
                .is_some_and(|w| w.contains("only the deploy projects")))
    );
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn published_ports_are_open_to_anyone_on_a_service_address(pool: Pool) {
    let mock = MockIncus::default();
    let provider = Provider::Mock(mock.clone());
    let state = AppState::new(provider.clone(), Auth::Tokens(pool.clone()))
        .with_network_uplink(Some("incusbr0".into()))
        .with_service_range(Some("198.18.100.0/29".into()));
    let operator = apelora_persistence::create_token(&pool, "op", "operator", None)
        .await
        .expect("token")
        .0;
    let body = json!({ "project": "apelora", "document": {},
        "modules": [module("web-01"), module("db-01")],
        "connections": [{ "from": "web-01", "to": "db-01", "transport": "tcp", "port": 5432 }],
        "published": [{ "module": "web-01", "transport": "tcp", "port": 80 }] });

    let (_, plan) = send(
        state.clone(),
        "POST",
        "/api/v1/applications/shop/plan",
        &operator,
        Some(body.clone()),
        None,
    )
    .await;
    assert!(
        plan["warnings"]
            .as_array()
            .expect("warnings")
            .iter()
            .any(|w| w
                .as_str()
                .is_some_and(|w| w.starts_with("published to outside clients: web-01"))),
        "{plan}"
    );
    let mut bad = body.clone();
    bad["published"] = json!([{ "module": "nope-01", "transport": "tcp", "port": 80 }]);
    let (status, _) = send(
        state.clone(),
        "POST",
        "/api/v1/applications/shop/plan",
        &operator,
        Some(bad),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = send(
        state.clone(),
        "POST",
        "/api/v1/applications/shop/deployments",
        &operator,
        Some(body),
        Some("pub-1"),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    drain(&pool, &provider).await;
    {
        let net = mock.net();
        // The single web instance gets a service address because it is published…
        let (_, lb) = net
            .load_balancers
            .get("apelora/shop/198.18.100.1")
            .expect("service address");
        assert_eq!(
            (lb.module.as_str(), lb.ports.clone()),
            ("web-01", vec![("tcp".to_owned(), 80)])
        );
        // …and its ACL lets anyone reach port 80, while the database stays reachable from web only.
        let (web_in, _) = &net.acls["apelora/shop-web-01"];
        assert!(
            web_in
                .iter()
                .any(|r| r.source.is_empty() && r.destination_port == "80")
        );
        let (db_in, _) = &net.acls["apelora/shop-db-01"];
        assert!(db_in.iter().all(|r| r.source == "$shop-web-01"));
    }
    let records = apelora_persistence::list_deployments(&pool)
        .await
        .expect("records");
    assert_eq!(
        records[0].published,
        Some(json!([{ "module": "web-01", "transport": "tcp", "port": 80 }]))
    );
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn internet_access_opens_only_external_destinations(pool: Pool) {
    let mock = MockIncus::default();
    let provider = Provider::Mock(mock.clone());
    let state = AppState::new(provider.clone(), Auth::Tokens(pool.clone()))
        .with_network_uplink(Some("incusbr0".into()));
    let operator = apelora_persistence::create_token(&pool, "op", "operator", None)
        .await
        .expect("token")
        .0;
    let mut web = module("web-01");
    web["internet"] = json!("web");
    let body = json!({ "project": "apelora", "document": {}, "modules": [web, module("db-01")],
        "connections": [{ "from": "web-01", "to": "db-01", "transport": "tcp", "port": 5432 }] });
    let (status, _) = send(
        state.clone(),
        "POST",
        "/api/v1/applications/shop/deployments",
        &operator,
        Some(body.clone()),
        Some("net-1"),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    drain(&pool, &provider).await;
    {
        let net = mock.net();
        let (_, web_out) = &net.acls["apelora/shop-web-01"];
        let external: Vec<&str> = web_out
            .iter()
            .filter(|r| r.destination == "@external")
            .map(|r| r.destination_port.as_str())
            .collect();
        assert_eq!(external, ["80,443", "53", "53"]);
        assert!(
            web_out.iter().all(|r| !r.destination.is_empty()),
            "never an any-destination egress rule"
        );
        let (_, db_out) = &net.acls["apelora/shop-db-01"];
        assert!(db_out.is_empty(), "no internet unless asked for");
    }
    // Without an uplink nothing is enforced, and the plan says so.
    let plain = AppState::new(provider, Auth::Tokens(pool.clone()));
    let (_, plan) = send(
        plain,
        "POST",
        "/api/v1/applications/shop/plan",
        &operator,
        Some(body),
        None,
    )
    .await;
    assert!(
        plan["warnings"]
            .as_array()
            .expect("warnings")
            .iter()
            .any(|w| w
                .as_str()
                .is_some_and(|w| w.starts_with("internet access is not enforced for web-01"))),
        "{plan}"
    );
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn consumers_are_created_knowing_where_their_providers_are(pool: Pool) {
    let provider = Provider::Mock(MockIncus::default());
    let state = AppState::new(provider.clone(), Auth::Tokens(pool.clone()))
        .with_network_uplink(Some("incusbr0".into()))
        .with_service_range(Some("198.18.100.0/29".into()));
    let operator = apelora_persistence::create_token(&pool, "op", "operator", None)
        .await
        .expect("token")
        .0;
    let mut web = module("web-01");
    web["replicas"] = json!(2);
    web["availability"] = json!("stateless");
    let body = json!({ "project": "apelora", "document": {},
        "modules": [module("client-01"), web, module("db-01")],
        "connections": [
            { "from": "client-01", "to": "web-01", "transport": "tcp", "port": 80 },
            { "from": "client-01", "to": "db-01", "transport": "tcp", "port": 5432 }] });
    let (status, _) = send(
        state.clone(),
        "POST",
        "/api/v1/applications/shop/deployments",
        &operator,
        Some(body),
        Some("disc-1"),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let jobs = apelora_persistence::recent_jobs(&pool, 20)
        .await
        .expect("jobs");
    let client = jobs
        .iter()
        .find(|j| j.kind == "instance.create" && j.payload["name"] == "shop-client-01")
        .expect("client job");
    assert_eq!(
        client.payload["config"]["environment.APELORA_SERVICE_WEB_01"], "198.18.100.1",
        "the service address"
    );
    assert_eq!(
        client.payload["config"]["environment.APELORA_SERVICE_DB_01"], "shop-db-01",
        "no service address: the instance name"
    );
    let web = jobs
        .iter()
        .find(|j| j.kind == "instance.create" && j.payload["name"] == "shop-web-01")
        .expect("web job");
    assert!(
        web.payload["config"]
            .as_object()
            .is_none_or(|c| !c.keys().any(|k| k.starts_with("environment.APELORA_"))),
        "providers learn nothing"
    );
    drain(&pool, &provider).await;

    // A design cannot forge them.
    let mut forged = module("client-01");
    forged["spec"]["config"] = json!({ "environment.APELORA_SERVICE_WEB_01": "203.0.113.9" });
    let (status, _) = send(
        state,
        "POST",
        "/api/v1/applications/shop/plan",
        &operator,
        Some(json!({ "project": "apelora", "modules": [forged] })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// ---- Machines and enrolment (Phase 09) -----------------------------------------------------

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn a_machine_is_registered_approved_joined_held_admitted_and_audited(pool: Pool) {
    let mock = Provider::Mock(MockIncus::default());
    let state =
        AppState::new(mock.clone(), Auth::Tokens(pool.clone())).with_admin(Some(mock.clone()));
    let admin = apelora_persistence::create_token(&pool, "admin", "fabric-admin", None)
        .await
        .expect("token")
        .0;
    let operator = apelora_persistence::create_token(&pool, "op", "operator", None)
        .await
        .expect("token")
        .0;
    let Provider::Mock(incus) = &mock else {
        unreachable!()
    };
    let state_of = |list: &Value, name: &str| {
        list["items"]
            .as_array()
            .and_then(|a| a.iter().find(|m| m["name"] == name))
            .map(|m| m["state"].as_str().unwrap_or_default().to_owned())
    };
    let register = json!({ "name": "mock-member-04", "description": "rack 2, slot 7" });

    // Operators cannot enrol machines; only manual machines can be registered for now.
    let (status, _) = send(
        state.clone(),
        "POST",
        "/api/v1/machines",
        &operator,
        Some(register.clone()),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, err) = send(
        state.clone(),
        "POST",
        "/api/v1/machines",
        &admin,
        Some(json!({ "name": "m9", "provider": "maas" })),
        None,
    )
    .await;
    assert_eq!(
        (status, err["error"]["code"].as_str()),
        (StatusCode::BAD_REQUEST, Some("provisioner_not_configured"))
    );

    let (status, m) = send(
        state.clone(),
        "POST",
        "/api/v1/machines",
        &admin,
        Some(register.clone()),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{m}");
    assert_eq!(
        (m["state"].as_str(), m["provider"].as_str()),
        (Some("registered"), Some("manual"))
    );
    assert_eq!(m["capabilities"]["installs"], false);
    let (status, _) = send(
        state.clone(),
        "POST",
        "/api/v1/machines",
        &admin,
        Some(register),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    // An existing member is adopted as already admitted.
    let (_, adopted) = send(
        state.clone(),
        "POST",
        "/api/v1/machines",
        &admin,
        Some(json!({ "name": "mock-member-01" })),
        None,
    )
    .await;
    assert_eq!(adopted["state"], "eligible");
    assert_eq!(adopted["warnings"], json!([]));

    // Not joined yet: no admission checks.
    let (status, err) = send(
        state.clone(),
        "GET",
        "/api/v1/machines/mock-member-04/admission",
        &admin,
        None,
        None,
    )
    .await;
    assert_eq!(
        (status, err["error"]["code"].as_str()),
        (StatusCode::CONFLICT, Some("not_a_member"))
    );

    // Approval: the token once, with the command; a second approval waits for a withdrawal.
    let approve = "/api/v1/machines/mock-member-04/approve";
    let (status, approved) = send(state.clone(), "POST", approve, &admin, None, None).await;
    assert_eq!(status, StatusCode::CREATED, "{approved}");
    let secret = approved["token"].as_str().expect("token").to_owned();
    assert!(
        approved["enrolCommand"]
            .as_str()
            .is_some_and(|c| c.contains("--name mock-member-04") && !c.contains(&secret))
    );
    let (status, err) = send(state.clone(), "POST", approve, &admin, None, None).await;
    assert_eq!(
        (status, err["error"]["code"].as_str()),
        (StatusCode::CONFLICT, Some("approval_pending"))
    );
    let (_, list) = send(state.clone(), "GET", "/api/v1/machines", &admin, None, None).await;
    assert_eq!(
        state_of(&list, "mock-member-04").as_deref(),
        Some("approved")
    );
    assert!(!list.to_string().contains(&secret));

    // Withdrawn, then approved again.
    let (status, _) = send(
        state.clone(),
        "DELETE",
        "/api/v1/machines/mock-member-04/approval",
        &admin,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, list) = send(state.clone(), "GET", "/api/v1/machines", &admin, None, None).await;
    assert_eq!(
        state_of(&list, "mock-member-04").as_deref(),
        Some("registered")
    );
    let (status, _) = send(state.clone(), "POST", approve, &admin, None, None).await;
    assert_eq!(status, StatusCode::CREATED);

    // The server joins and holds itself (the enrolment script does this).
    assert!(incus.join("mock-member-04", true));
    let (_, list) = send(state.clone(), "GET", "/api/v1/machines", &admin, None, None).await;
    assert_eq!(state_of(&list, "mock-member-04").as_deref(), Some("held"));
    let (status, checks) = send(
        state.clone(),
        "GET",
        "/api/v1/machines/mock-member-04/admission",
        &admin,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{checks}");
    assert_eq!(checks["admissible"], true, "{checks}");
    let (status, err) = send(
        state.clone(),
        "DELETE",
        "/api/v1/machines/mock-member-04",
        &admin,
        None,
        None,
    )
    .await;
    assert_eq!(
        (status, err["error"]["code"].as_str()),
        (StatusCode::CONFLICT, Some("still_a_member"))
    );

    let (status, admitted) = send(
        state.clone(),
        "POST",
        "/api/v1/machines/mock-member-04/admit",
        &admin,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{admitted}");
    let (_, list) = send(state.clone(), "GET", "/api/v1/machines", &admin, None, None).await;
    assert_eq!(
        state_of(&list, "mock-member-04").as_deref(),
        Some("eligible")
    );
    let members = mock.cluster_members().await.expect("members");
    assert_eq!(
        members
            .iter()
            .find(|m| m.name == "mock-member-04")
            .map(|m| m.scheduler.as_str()),
        Some("all")
    );

    let audit: Vec<(String, String, String)> =
        sqlx::query_as("SELECT action, target, detail::text FROM audit_events WHERE action <> 'access.denied' ORDER BY id")
            .fetch_all(&pool)
            .await
            .expect("audit");
    let actions: Vec<&str> = audit.iter().map(|(a, _, _)| a.as_str()).collect();
    assert_eq!(
        actions,
        [
            "machine.register",
            "machine.register",
            "machine.approve",
            "machine.approval.withdraw",
            "machine.approve",
            "machine.admit"
        ]
    );
    assert!(audit.iter().all(|(_, _, d)| !d.contains(&secret)));
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn a_failing_check_blocks_admission_and_a_member_that_skipped_the_hold_is_flagged(
    pool: Pool,
) {
    let mock = Provider::Mock(MockIncus::default());
    let state =
        AppState::new(mock.clone(), Auth::Tokens(pool.clone())).with_admin(Some(mock.clone()));
    let admin = apelora_persistence::create_token(&pool, "admin", "fabric-admin", None)
        .await
        .expect("token")
        .0;
    let Provider::Mock(incus) = &mock else {
        unreachable!()
    };
    for name in ["mock-member-04", "mock-member-05"] {
        send(
            state.clone(),
            "POST",
            "/api/v1/machines",
            &admin,
            Some(json!({ "name": name })),
            None,
        )
        .await;
        let path = format!("/api/v1/machines/{name}/approve");
        assert_eq!(
            send(state.clone(), "POST", &path, &admin, None, None)
                .await
                .0,
            StatusCode::CREATED
        );
    }
    // 04 joins held, then goes offline before it is admitted.
    assert!(incus.join("mock-member-04", true));
    incus
        .members
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter_mut()
        .filter(|m| m.name == "mock-member-04")
        .for_each(|m| m.status = "Offline".into());
    let (status, err) = send(
        state.clone(),
        "POST",
        "/api/v1/machines/mock-member-04/admit",
        &admin,
        None,
        None,
    )
    .await;
    assert_eq!(
        (status, err["error"]["code"].as_str()),
        (StatusCode::CONFLICT, Some("admission_blocked"))
    );
    assert!(
        err["error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("Member online: Offline")),
        "{err}"
    );
    let members = mock.cluster_members().await.expect("members");
    assert_eq!(
        members
            .iter()
            .find(|m| m.name == "mock-member-04")
            .map(|m| m.scheduler.as_str()),
        Some(apelora_domain::enrolment::HELD)
    );

    // 05 joins without holding itself: eligible, with a warning.
    assert!(incus.join("mock-member-05", false));
    let (_, list) = send(state.clone(), "GET", "/api/v1/machines", &admin, None, None).await;
    let m05 = list["items"]
        .as_array()
        .and_then(|a| a.iter().find(|m| m["name"] == "mock-member-05"))
        .cloned()
        .expect("listed");
    assert_eq!(m05["state"], "eligible");
    assert!(
        m05["warnings"][0]
            .as_str()
            .is_some_and(|w| w.contains("without being held"))
    );
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn status_reports_the_health_the_worker_decided_for_running_instances(pool: Pool) {
    let provider = Provider::Mock(MockIncus::default());
    let state = AppState::new(provider.clone(), Auth::Tokens(pool.clone()));
    let operator = apelora_persistence::create_token(&pool, "op", "operator", None)
        .await
        .expect("token")
        .0;
    let mut web = module("web-01");
    web["replicas"] = json!(2);
    web["availability"] = json!("stateless");
    let body = json!({ "project": "apelora", "document": {}, "modules": [web] });
    let (status, _) = send(
        state.clone(),
        "POST",
        "/api/v1/applications/shop/deployments",
        &operator,
        Some(body),
        Some("health-1"),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    drain(&pool, &provider).await;
    let app = apelora_persistence::ensure_application(&pool, "shop")
        .await
        .expect("app");
    let since = time::OffsetDateTime::now_utc();
    for (instance, health, detail) in [
        ("shop-web-01", "healthy", "port check passing"),
        ("shop-web-01-r2", "unhealthy", "port check failing"),
    ] {
        apelora_persistence::record_health(
            &pool,
            &apelora_persistence::NewHealth {
                application_id: app.id,
                project: "apelora",
                instance,
                module: "web-01",
                health,
                since,
                detail,
                restarted_at: (health == "unhealthy").then_some(since),
            },
        )
        .await
        .expect("record");
    }
    let (status, body) = send(
        state.clone(),
        "GET",
        "/api/v1/applications/shop/status",
        &operator,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let health = |instance: &str| {
        body["items"]
            .as_array()
            .and_then(|a| a.iter().find(|i| i["instance"] == instance))
            .map(|i| i["health"].clone())
            .expect("listed")
    };
    assert_eq!(health("shop-web-01")["health"], "healthy");
    assert!(health("shop-web-01")["restartedAt"].is_null());
    assert_eq!(
        (
            health("shop-web-01-r2")["health"].as_str(),
            health("shop-web-01-r2")["detail"].as_str()
        ),
        (Some("unhealthy"), Some("port check failing"))
    );
    assert!(health("shop-web-01-r2")["restartedAt"].is_string());

    // A stopped instance's recorded health is not current: not reported.
    provider
        .change_state(
            "apelora",
            "shop-web-01-r2",
            apelora_domain::InstanceAction::Stop,
            true,
        )
        .await
        .expect("stop");
    let (_, body) = send(
        state,
        "GET",
        "/api/v1/applications/shop/status",
        &operator,
        None,
        None,
    )
    .await;
    let r2 = body["items"]
        .as_array()
        .and_then(|a| a.iter().find(|i| i["instance"] == "shop-web-01-r2"))
        .expect("listed");
    assert!(r2.get("health").is_none(), "{r2}");
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn removing_every_module_without_connections_still_removes_the_network(pool: Pool) {
    let mock = MockIncus::default();
    let provider = Provider::Mock(mock.clone());
    let state = AppState::new(provider.clone(), Auth::Tokens(pool.clone()))
        .with_network_uplink(Some("incusbr0".into()));
    let operator = apelora_persistence::create_token(&pool, "op", "operator", None)
        .await
        .expect("token")
        .0;
    let conn = json!([{ "from": "web-01", "to": "db-01", "transport": "tcp", "port": 5432 }]);
    let body = json!({ "project": "apelora", "document": {}, "modules": [module("web-01"), module("db-01")], "connections": conn });
    let (status, _) = send(
        state.clone(),
        "POST",
        "/api/v1/applications/shop/deployments",
        &operator,
        Some(body),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    drain(&pool, &provider).await;
    assert!(mock.net().networks.contains("apelora/shop"));

    // A removal that leaves `connections` out (it has nothing left to connect).
    let body =
        json!({ "project": "apelora", "document": {}, "modules": [], "removeMissing": true });
    let (status, d) = send(
        state,
        "POST",
        "/api/v1/applications/shop/deployments",
        &operator,
        Some(body),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{d}");
    assert!(d["networkJob"].is_string(), "{d}");
    drain(&pool, &provider).await;
    let net = mock.net().clone();
    assert!(!net.networks.contains("apelora/shop"), "{:?}", net.networks);
    assert!(net.acls.keys().all(|k| !k.contains("shop")));
    assert!(net.address_sets.keys().all(|k| !k.contains("shop")));
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn volumes_go_to_the_configured_pool_and_a_design_never_names_one(pool: Pool) {
    let Provider::Mock(m) = Provider::Mock(MockIncus::default()) else {
        unreachable!()
    };
    let provider = Provider::Mock(m.clone());
    let operator = apelora_persistence::create_token(&pool, "op", "operator", None)
        .await
        .expect("token")
        .0;
    let design = |pool_field: Option<&str>| {
        let mut db = module("db-01");
        db["availability"] = json!("stateful");
        let mut volume = json!({ "name": "data", "sizeMib": 2048, "path": "/var/lib/postgresql" });
        if let Some(p) = pool_field {
            volume["pool"] = json!(p);
        }
        db["spec"]["volumes"] = json!([volume]);
        json!({ "project": "apelora", "document": {}, "modules": [db] })
    };
    let plan = |state: AppState, body: Value| {
        let operator = operator.clone();
        async move {
            send(
                state,
                "POST",
                "/api/v1/applications/shop/plan",
                &operator,
                Some(body),
                None,
            )
            .await
        }
    };

    // No pool for the class: refused, with the reason.
    let unconfigured = AppState::new(provider.clone(), Auth::Tokens(pool.clone()));
    let (status, body) = plan(unconfigured, design(None)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("APELORA_VOLUME_POOL"),
        "{body}"
    );

    // A configured pool Incus does not have: refused, never a fallback.
    let missing = AppState::new(provider.clone(), Auth::Tokens(pool.clone()))
        .with_volume_pool(Some("fast-ssd".into()));
    let (status, body) = plan(missing, design(None)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("fast-ssd does not exist"),
        "{body}"
    );

    let state = AppState::new(provider.clone(), Auth::Tokens(pool.clone()))
        .with_volume_pool(Some("mock-local".into()));
    // A design that names a pool is refused: pools are the administrator's choice.
    let (status, body) = plan(state.clone(), design(Some("mock-local"))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    // Stateless modules cannot have volumes.
    let mut stateless = design(None);
    stateless["modules"][0]["availability"] = json!("stateless");
    let (status, _) = plan(state.clone(), stateless).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, body) = send(
        state.clone(),
        "POST",
        "/api/v1/applications/shop/deployments",
        &operator,
        Some(design(None)),
        Some("volumes-1"),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    drain(&pool, &provider).await;
    let db = provider
        .instance("apelora", "shop-db-01")
        .await
        .expect("read")
        .expect("created");
    assert_eq!(db.volume_devices, vec!["vol-data".to_owned()]);
    let volume = provider
        .volume(
            "apelora",
            "mock-local",
            Some(&db.location),
            "shop-db-01-data",
        )
        .await
        .expect("read")
        .expect("created in the configured pool");
    assert_eq!(volume.size_mib, Some(2048));

    // A direct instance request cannot carry volumes.
    let mut spec = web_spec("apelora", "lone-01");
    spec["volumes"] = json!([{ "name": "data", "sizeMib": 128, "path": "/data" }]);
    let (status, _) = send(
        state,
        "POST",
        "/api/v1/instances",
        &operator,
        Some(spec),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn volumes_are_listed_kept_after_removal_and_deleted_only_on_purpose(pool: Pool) {
    let provider = Provider::Mock(MockIncus::default());
    let state = AppState::new(provider.clone(), Auth::Tokens(pool.clone()))
        .with_volume_pool(Some("mock-local".into()));
    let operator = apelora_persistence::create_token(&pool, "op", "operator", None)
        .await
        .expect("token")
        .0;
    let designer = apelora_persistence::create_token(&pool, "d", "studio-designer", None)
        .await
        .expect("token")
        .0;
    let mut db = module("db-01");
    db["availability"] = json!("stateful");
    db["spec"]["volumes"] =
        json!([{ "name": "data", "sizeMib": 1024, "path": "/var/lib/postgresql" }]);
    let deploy = |modules: Value, key: &'static str| {
        let (state, operator) = (state.clone(), operator.clone());
        async move {
            send(state, "POST", "/api/v1/applications/shop/deployments", &operator,
                Some(json!({ "project": "apelora", "document": {}, "modules": modules, "removeMissing": true })),
                Some(key)).await
        }
    };
    let (status, body) = deploy(json!([db]), "vol-a").await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    drain(&pool, &provider).await;

    let list = |token: String| {
        let state = state.clone();
        async move {
            send(
                state,
                "GET",
                "/api/v1/applications/shop/volumes",
                &token,
                None,
                None,
            )
            .await
        }
    };
    let (status, body) = list(operator.clone()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["source"]["provider"], "mock");
    let item = &body["items"][0];
    assert_eq!(
        (
            item["incusName"].as_str(),
            item["state"].as_str(),
            item["sizeMib"].as_i64()
        ),
        (Some("shop-db-01-data"), Some("attached"), Some(1024)),
        "{body}"
    );
    assert_eq!(item["usedBy"], json!(["shop-db-01"]));

    let delete = |token: String, confirm: &'static str| {
        let state = state.clone();
        async move {
            send(
                state,
                "DELETE",
                "/api/v1/applications/shop/volumes/shop-db-01-data",
                &token,
                Some(json!({ "confirm": confirm })),
                None,
            )
            .await
        }
    };
    // In use: refused, nothing deleted.
    let (status, body) = delete(operator.clone(), "shop-db-01-data").await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");

    // Removing the module deletes the instance and keeps the data.
    let (status, _) = deploy(json!([]), "vol-b").await;
    assert_eq!(status, StatusCode::ACCEPTED);
    drain(&pool, &provider).await;
    assert_eq!(provider.instance("apelora", "shop-db-01").await, Ok(None));
    let (_, body) = list(operator.clone()).await;
    assert_eq!(body["items"][0]["state"], "retained", "{body}");

    // Deleting needs the name typed again and the right to delete.
    let (status, _) = delete(operator.clone(), "shop-db-01").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = delete(designer, "shop-db-01-data").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = delete(operator.clone(), "shop-db-01-data").await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let (_, body) = list(operator.clone()).await;
    assert_eq!(body["items"], json!([]));
    let Provider::Mock(m) = &provider else {
        unreachable!()
    };
    assert!(m.storage().volumes.is_empty());
    let audited: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE action = 'application.volume.delete' AND target = 'apelora/shop-db-01-data'",
    )
    .fetch_one(&pool)
    .await
    .expect("audit");
    assert_eq!(audited, 1);
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn volumes_grow_but_never_shrink(pool: Pool) {
    let provider = Provider::Mock(MockIncus::default());
    let state = AppState::new(provider.clone(), Auth::Tokens(pool.clone()))
        .with_volume_pool(Some("mock-local".into()));
    let operator = apelora_persistence::create_token(&pool, "op", "operator", None)
        .await
        .expect("token")
        .0;
    let auditor = apelora_persistence::create_token(&pool, "a", "auditor", None)
        .await
        .expect("token")
        .0;
    let mut db = module("db-01");
    db["availability"] = json!("stateful");
    db["spec"]["volumes"] = json!([{ "name": "data", "sizeMib": 1024, "path": "/data" }]);
    let (status, _) = send(
        state.clone(),
        "POST",
        "/api/v1/applications/shop/deployments",
        &operator,
        Some(json!({ "project": "apelora", "document": {}, "modules": [db] })),
        Some("grow-1"),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    drain(&pool, &provider).await;
    let grow = |token: String, size: u32| {
        let state = state.clone();
        async move {
            send(
                state,
                "PATCH",
                "/api/v1/applications/shop/volumes/shop-db-01-data",
                &token,
                Some(json!({ "sizeMib": size })),
                None,
            )
            .await
        }
    };
    let (status, _) = grow(auditor, 2048).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = grow(operator.clone(), 512).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("only grow"),
        "{body}"
    );
    let (status, body) = grow(operator.clone(), 4096).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let (_, list) = send(
        state.clone(),
        "GET",
        "/api/v1/applications/shop/volumes",
        &operator,
        None,
        None,
    )
    .await;
    assert_eq!(list["items"][0]["sizeMib"], 4096);
    let db = provider
        .instance("apelora", "shop-db-01")
        .await
        .expect("read")
        .expect("exists");
    let live = provider
        .volume(
            "apelora",
            "mock-local",
            Some(&db.location),
            "shop-db-01-data",
        )
        .await
        .expect("read")
        .expect("volume");
    assert_eq!(live.size_mib, Some(4096));
    let audited: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE action = 'application.volume.grow'",
    )
    .fetch_one(&pool)
    .await
    .expect("audit");
    assert_eq!(audited, 1);
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn a_member_is_removed_only_when_evacuated_empty_and_confirmed(pool: Pool) {
    let Provider::Mock(m) = Provider::Mock(MockIncus::default()) else {
        unreachable!()
    };
    let mock = Provider::Mock(m.clone());
    let state =
        AppState::new(mock.clone(), Auth::Tokens(pool.clone())).with_admin(Some(mock.clone()));
    let admin = apelora_persistence::create_token(&pool, "admin", "fabric-admin", None)
        .await
        .expect("token")
        .0;
    let operator = apelora_persistence::create_token(&pool, "op", "operator", None)
        .await
        .expect("token")
        .0;
    let preview = |member: &'static str| {
        let (state, operator) = (state.clone(), operator.clone());
        async move {
            send(
                state,
                "GET",
                &format!("/api/v1/cluster/members/{member}/removal"),
                &operator,
                None,
                None,
            )
            .await
        }
    };
    let remove = |token: String, member: &'static str, confirm: &'static str| {
        let state = state.clone();
        async move {
            send(
                state,
                "POST",
                &format!("/api/v1/cluster/members/{member}/remove"),
                &token,
                Some(json!({ "confirm": confirm })),
                None,
            )
            .await
        }
    };

    // Online, with instances on it: blocked, with the reasons.
    let (status, body) = preview("mock-member-03").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let blocked = body["plan"]["blocked"].to_string();
    assert!(
        blocked.contains("evacuate it first") && blocked.contains("still on it"),
        "{body}"
    );
    assert_eq!(body["plan"]["instances"], json!(["default/mock-db-01"]));
    let (status, body) = remove(admin.clone(), "mock-member-03", "mock-member-03").await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");

    // A fourth member, evacuated and empty: only an administrator removes it, with its name typed.
    mock.create_join_token("mock-member-04")
        .await
        .expect("token");
    assert!(m.join("mock-member-04", false));
    mock.member_state("mock-member-04", false)
        .await
        .expect("evacuate");
    let (_, body) = preview("mock-member-04").await;
    assert_eq!(body["plan"]["blocked"], json!([]), "{body}");
    let (status, _) = remove(operator.clone(), "mock-member-04", "mock-member-04").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = remove(admin.clone(), "mock-member-04", "mock-member-4").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, body) = remove(admin.clone(), "mock-member-04", "mock-member-04").await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert!(
        !m.members
            .lock()
            .expect("members")
            .iter()
            .any(|x| x.name == "mock-member-04")
    );
    let audited: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_events WHERE action = 'cluster.member.remove' AND target = 'mock-member-04'")
        .fetch_one(&pool)
        .await
        .expect("audit");
    assert_eq!(audited, 1);
    let (status, _) = preview("mock-member-04").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn snapshots_are_taken_listed_restored_only_when_stopped_and_deleted(pool: Pool) {
    let Provider::Mock(m) = Provider::Mock(MockIncus::default()) else {
        unreachable!()
    };
    let provider = Provider::Mock(m.clone());
    let state = AppState::new(provider.clone(), Auth::Tokens(pool.clone()))
        .with_volume_pool(Some("mock-local".into()));
    let operator = apelora_persistence::create_token(&pool, "op", "operator", None)
        .await
        .expect("token")
        .0;
    let mut db = module("db-01");
    db["availability"] = json!("stateful");
    db["spec"]["volumes"] = json!([{ "name": "data", "sizeMib": 1024, "path": "/data" }]);
    let (status, _) = send(
        state.clone(),
        "POST",
        "/api/v1/applications/shop/deployments",
        &operator,
        Some(json!({ "project": "apelora", "document": {}, "modules": [db] })),
        Some("snap-1"),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    drain(&pool, &provider).await;
    let base = "/api/v1/applications/shop/volumes/shop-db-01-data/snapshots";

    let (status, taken) = send(state.clone(), "POST", base, &operator, None, None).await;
    assert_eq!(status, StatusCode::CREATED, "{taken}");
    assert_eq!(
        taken["consistency"], "crash-consistent",
        "the instance was running"
    );
    let snap = taken["name"].as_str().expect("name").to_owned();
    assert!(snap.starts_with("snap-"), "{snap}");
    let (_, list) = send(state.clone(), "GET", base, &operator, None, None).await;
    assert_eq!(list["items"][0]["name"], snap.as_str(), "{list}");

    let restore = |confirm: String| {
        let (state, operator, path) = (
            state.clone(),
            operator.clone(),
            format!("{base}/{snap}/restore"),
        );
        async move {
            send(
                state,
                "POST",
                &path,
                &operator,
                Some(json!({ "confirm": confirm })),
                None,
            )
            .await
        }
    };
    let (status, body) = restore(snap.clone()).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("stop shop-db-01 first"),
        "{body}"
    );
    provider
        .change_state(
            "apelora",
            "shop-db-01",
            apelora_domain::InstanceAction::Stop,
            false,
        )
        .await
        .expect("stop");
    let (status, _) = restore("snap-wrong".into()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, body) = restore(snap.clone()).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert_eq!(
        m.storage().restores,
        vec![format!("shop-db-01-data/{snap}")]
    );

    let (status, _) = send(
        state.clone(),
        "DELETE",
        &format!("{base}/{snap}"),
        &operator,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, list) = send(state.clone(), "GET", base, &operator, None, None).await;
    assert_eq!(list["items"], json!([]));
    let actions: Vec<String> = sqlx::query_scalar(
        "SELECT action FROM audit_events WHERE action LIKE 'application.volume.%' ORDER BY id",
    )
    .fetch_all(&pool)
    .await
    .expect("audit");
    assert_eq!(
        actions,
        [
            "application.volume.snapshot",
            "application.volume.restore",
            "application.volume.snapshot.delete"
        ]
    );
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn an_instance_is_moved_after_a_dry_run_with_its_volume_and_a_stop_only_when_allowed(
    pool: Pool,
) {
    let provider = Provider::Mock(MockIncus::default());
    let state = AppState::new(provider.clone(), Auth::Tokens(pool.clone()))
        .with_volume_pool(Some("mock-local".into()));
    let operator = apelora_persistence::create_token(&pool, "op", "operator", None)
        .await
        .expect("token")
        .0;
    let mut db = module("db-01");
    db["availability"] = json!("stateful");
    db["spec"]["volumes"] = json!([{ "name": "data", "sizeMib": 1024, "path": "/data" }]);
    let (status, _) = send(
        state.clone(),
        "POST",
        "/api/v1/applications/shop/deployments",
        &operator,
        Some(json!({ "project": "apelora", "document": {}, "modules": [db] })),
        Some("mv-1"),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    drain(&pool, &provider).await;
    let base = "/api/v1/instances/apelora/shop-db-01/move";

    let (status, body) = send(
        state.clone(),
        "GET",
        &format!("{base}?target=mock-member-02"),
        &operator,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body["plan"]["blocked"]
            .to_string()
            .contains("allow a short stop"),
        "{body}"
    );
    let (_, body) = send(
        state.clone(),
        "GET",
        &format!("{base}?target=mock-member-02&allowStop=true"),
        &operator,
        None,
        None,
    )
    .await;
    assert_eq!(
        (
            body["plan"]["stops"].as_bool(),
            body["plan"]["live"].as_bool()
        ),
        (Some(true), Some(false)),
        "{body}"
    );
    assert_eq!(body["plan"]["volumes"], json!(["shop-db-01-data"]));

    let (status, _) = send(
        state.clone(),
        "POST",
        base,
        &operator,
        Some(json!({ "target": "mock-member-02", "confirm": "shop-db", "allowStop": true })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, body) = send(
        state.clone(),
        "POST",
        base,
        &operator,
        Some(json!({ "target": "mock-member-02", "confirm": "shop-db-01" })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let (status, body) = send(
        state.clone(),
        "POST",
        base,
        &operator,
        Some(json!({ "target": "mock-member-02", "confirm": "shop-db-01", "allowStop": true })),
        Some("mv-2"),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    drain(&pool, &provider).await;
    let now = provider
        .instance("apelora", "shop-db-01")
        .await
        .expect("read")
        .expect("exists");
    assert_eq!(
        (now.location.as_str(), now.status.as_str()),
        ("mock-member-02", "Running")
    );
    let (_, list) = send(
        state,
        "GET",
        "/api/v1/applications/shop/volumes",
        &operator,
        None,
        None,
    )
    .await;
    assert_eq!(list["items"][0]["member"], "mock-member-02", "{list}");
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn sign_in_options_are_public_and_name_the_identity_provider_only_when_set_up(pool: Pool) {
    let provider = Provider::Mock(MockIncus::default());
    let plain = AppState::new(provider.clone(), Auth::Tokens(pool.clone()));
    let (status, body, _) = call(plain, "/api/v1/sign-in", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({ "providers": [], "apiTokens": true, "localAccounts": false, "directories": [] })
    );

    let with_oidc = AppState::new(provider, Auth::Tokens(pool))
        .with_legacy_tokens(false)
        .with_sign_in(Some(super::OidcSignIn {
            issuer: "https://id.example.test/realms/apelora".into(),
            client_id: "apelora-console".into(),
            scopes: vec!["openid".into(), "profile".into()],
            label: "Example Corp".into(),
        }));
    let (status, body, _) = call(with_oidc, "/api/v1/sign-in", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["providers"][0]["clientId"], "apelora-console");
    assert_eq!(body["providers"][0]["label"], "Example Corp");
    assert_eq!(body["apiTokens"], false);
}
