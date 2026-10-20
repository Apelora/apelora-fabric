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
    assert_eq!(body["error"]["code"], "store_unavailable");
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
    ] {
        assert!(doc["paths"][route]["get"].is_object(), "{route} missing");
    }
    assert!(doc["paths"]["/api/v1/instances"]["post"].is_object());
    assert!(doc["paths"]["/api/v1/instances/{project}/{name}/state"]["post"].is_object());
    assert!(doc["paths"]["/api/v1/instances/{project}/{name}"]["delete"].is_object());
    assert!(doc["paths"]["/api/v1/cluster/join-tokens"]["post"].is_object());
    assert!(doc["paths"]["/api/v1/cluster/join-tokens"]["get"].is_object());
    assert!(doc["paths"]["/api/v1/cluster/join-tokens/{member}"]["delete"].is_object());
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

async fn send(
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

    let actions: Vec<String> = sqlx::query_scalar("SELECT action FROM audit_events ORDER BY id")
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
        (StatusCode::SERVICE_UNAVAILABLE, Some("store_unavailable"))
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
        sqlx::query_as("SELECT action, target, detail::text FROM audit_events ORDER BY id")
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
