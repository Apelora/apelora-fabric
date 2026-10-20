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

    let audit: Vec<(String, String)> =
        sqlx::query_as("SELECT action, detail::text FROM audit_events ORDER BY id")
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

fn appliance(id: &str) -> Value {
    json!({ "appliance": id, "spec": {
        "project": "ignored", "name": "ignored", "kind": "container", "start": true,
        "source": { "protocol": "simplestreams", "server": "https://images.example", "alias": "debian/13" } } })
}

async fn drain(pool: &Pool, provider: &Provider) {
    while apelora_deployment_worker::run_once(pool, provider, "w")
        .await
        .expect("worker")
    {}
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn applications_are_planned_deployed_revised_and_reported_by_appliance(pool: Pool) {
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
    let both =
        json!({ "project": "apelora", "appliances": [appliance("web-01"), appliance("db-01")] });

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
    let twice =
        json!({ "project": "apelora", "appliances": [appliance("web-01"), appliance("web-01")] });
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
                i["appliance"].as_str().unwrap_or_default().to_owned(),
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
    let mut one =
        json!({ "project": "apelora", "appliances": [appliance("web-01")], "document": doc });
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
    let clash =
        json!({ "project": "default", "appliances": [appliance("web-01")], "document": {} });
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
    let body = |connections: Value| json!({ "project": "apelora", "document": {}, "appliances": [appliance("web-01"), appliance("db-01"), appliance("cache-01")], "connections": connections });

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
        "unconnected appliances are isolated"
    );
    assert!(net.address_sets["apelora/shop-web-01"][0].starts_with("198.51.100."));
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn removed_appliances_lose_their_network_objects_and_other_applications_keep_theirs(
    pool: Pool,
) {
    let mock = MockIncus::default();
    let provider = Provider::Mock(mock.clone());
    let state = AppState::new(provider.clone(), Auth::Tokens(pool.clone()))
        .with_network_uplink(Some("incusbr0".into()));
    let operator = apelora_persistence::create_token(&pool, "op", "operator", None)
        .await
        .expect("token")
        .0;
    let conn = json!([{ "from": "web-01", "to": "db-01", "transport": "tcp", "port": 5432 }]);
    let deploy = |app: &'static str, appliances: Value, connections: Value| {
        let state = state.clone();
        let operator = operator.clone();
        async move {
            let body = json!({ "project": "apelora", "document": {}, "appliances": appliances, "connections": connections, "removeMissing": true });
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
        json!([appliance("web-01"), appliance("db-01")]),
        conn.clone(),
    )
    .await;
    // "shop-eu" starts with "shop-": a name prefix must never decide ownership.
    deploy(
        "shop-eu",
        json!([appliance("web-01"), appliance("db-01")]),
        conn,
    )
    .await;
    drain(&pool, &provider).await;
    let eu_before = mock
        .net()
        .acls
        .keys()
        .filter(|k| k.contains("shop-eu"))
        .count();
    assert_eq!(eu_before, 2);

    deploy("shop", json!([appliance("web-01")]), json!([])).await;
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
        "the network goes with the last appliance"
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
        let mut web = appliance("web-01");
        web["replicas"] = json!(2);
        web["availability"] = json!(availability);
        json!({ "project": "apelora", "document": {}, "appliances": [web, appliance("db-01")] })
    };

    // Only stateless appliances may have more than one replica.
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
async fn connected_stateless_appliances_get_a_service_address_until_removed(pool: Pool) {
    let mock = MockIncus::default();
    let provider = Provider::Mock(mock.clone());
    let state = AppState::new(provider.clone(), Auth::Tokens(pool.clone()))
        .with_network_uplink(Some("incusbr0".into()))
        .with_service_range(Some("198.18.100.0/29".into()));
    let operator = apelora_persistence::create_token(&pool, "op", "operator", None)
        .await
        .expect("token")
        .0;
    let mut web = appliance("web-01");
    web["replicas"] = json!(2);
    web["availability"] = json!("stateless");
    let deploy = |appliances: Value, connections: Value, key: &'static str| {
        let state = state.clone();
        let operator = operator.clone();
        async move {
            let (status, body) = send(
                state,
                "POST",
                "/api/v1/applications/shop/deployments",
                &operator,
                Some(
                    json!({ "project": "apelora", "document": {}, "appliances": appliances,
                             "connections": connections, "removeMissing": true }),
                ),
                Some(key),
            )
            .await;
            assert_eq!(status, StatusCode::ACCEPTED, "{body}");
        }
    };
    deploy(
        json!([appliance("client-01"), web.clone(), appliance("db-01")]),
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
    assert_eq!((owner.as_str(), lb.appliance.as_str()), ("shop", "web-01"));
    assert_eq!(lb.ports, vec![("tcp".to_owned(), 80)]);
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
        json!([{ "appliance": "web-01", "project": "apelora", "address": "198.18.100.1" }])
    );

    // web-01 leaves the design: its load balancer goes and its address is free again.
    deploy(
        json!([appliance("client-01"), appliance("db-01")]),
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
