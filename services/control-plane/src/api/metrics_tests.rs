// Metrics through the real router: who may scrape, what the exposition contains after a
// request and a job, and that no secret, name or id ends up in it. Evidence: MOCK VERIFIED
// (mock Incus) against a real PostgreSQL.

use apelora_incus_adapter::MockIncus;
use axum::body::Body;
use axum::http::Request;
use http_body_util::BodyExt;
use serde_json::json;
use tower::ServiceExt;
use uuid::Uuid;

use super::identity_tests::{lab, world};
use super::*;

/// GET `path` with an optional bearer token and `Accept` header; the body as text.
async fn get_text(
    state: AppState,
    path: &str,
    token: Option<&str>,
    accept: Option<&str>,
) -> (StatusCode, String, String) {
    let mut req = Request::get(path);
    if let Some(t) = token {
        req = req.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    if let Some(a) = accept {
        req = req.header(header::ACCEPT, a);
    }
    let res = router(state)
        .oneshot(req.body(Body::empty()).expect("request"))
        .await
        .expect("response");
    let status = res.status();
    let ct = res
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let bytes = res.into_body().collect().await.expect("body").to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned(), ct)
}

/// The value of the first sample whose line starts with `prefix` (name and labels).
fn sample(text: &str, name: &str, labels: &[(&str, &str)]) -> Option<f64> {
    text.lines()
        .filter(|l| l.starts_with(&format!("{name}{{")) || l.starts_with(&format!("{name} ")))
        .find(|l| {
            labels
                .iter()
                .all(|(k, v)| l.contains(&format!("{k}=\"{v}\"")))
        })
        .and_then(|l| l.rsplit(' ').next())
        .and_then(|v| v.parse().ok())
}

#[tokio::test]
async fn requests_are_counted_by_route_template_never_by_path() {
    let state = AppState::new(
        Provider::Mock(MockIncus::default()),
        Auth::DisabledForDevelopment,
    );
    get_text(
        state.clone(),
        "/api/v1/applications/secret-shop-name/status",
        None,
        None,
    )
    .await;
    get_text(state.clone(), "/api/v1/no-such-route/abc-123", None, None).await;
    get_text(state.clone(), "/api/v1/cluster", None, None).await;
    let (status, text, ct) = get_text(state, "/metrics", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(ct.starts_with("text/plain; version=0.0.4"), "{ct}");
    assert!(
        sample(
            &text,
            "apelora_http_requests_total",
            &[
                ("route", "/api/v1/cluster"),
                ("method", "GET"),
                ("status", "2xx")
            ]
        )
        .is_some_and(|v| v >= 1.0),
        "{text}"
    );
    assert!(
        text.contains("route=\"/api/v1/applications/{name}/status\""),
        "{text}"
    );
    assert!(text.contains("route=\"unmatched\""), "{text}");
    assert!(!text.contains("secret-shop-name"), "a name became a label");
    assert!(!text.contains("abc-123"), "a raw path became a label");
    assert!(text.contains("apelora_http_request_duration_seconds_bucket{"));
    assert!(
        text.contains("apelora_auth_attempts_total{") && text.contains("method=\"development\"")
    );
    // Inventory from the mock, labelled as such.
    assert!(text.contains("apelora_incus_members{"), "{text}");
    assert!(text.contains("apelora_provider_info"), "{text}");
}

#[tokio::test]
async fn openmetrics_is_answered_when_the_collector_asks_for_it() {
    let state = AppState::new(
        Provider::Mock(MockIncus::default()),
        Auth::DisabledForDevelopment,
    );
    let (status, text, ct) = get_text(
        state,
        "/metrics",
        None,
        Some("application/openmetrics-text; version=1.0.0,text/plain;q=0.5"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(ct.starts_with("application/openmetrics-text"), "{ct}");
    assert!(text.ends_with("# EOF\n"));
    assert!(text.contains("# TYPE apelora_http_requests counter"));
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn scraping_needs_the_metrics_permission_and_shows_no_secret_name_or_id(pool: Pool) {
    let l = lab(pool);
    let w = world(&l).await;
    // Nobody signed in: refused.
    let (s, _, _) = get_text(l.state.clone(), "/metrics", None, None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    // A person with no role: refused by the gate, and the denial is counted by permission.
    let nobody = l.token("sub-nobody", "nobody");
    let before = apelora_metrics::AUTHZ_DENIALS.get(&["fabric.metrics.read"]);
    let (s, _, _) = get_text(l.state.clone(), "/metrics", Some(&nobody), None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert!(apelora_metrics::AUTHZ_DENIALS.get(&["fabric.metrics.read"]) > before);
    // The collector: an automation identity with only the Metrics Reader role.
    let (s, ci) = l
        .req(
            "POST",
            "/api/v1/identity/service-identities",
            &w.admin,
            Some(json!({ "name": "metrics-collector", "kind": "automation" })),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{ci}");
    let token = ci["token"].as_str().expect("token").to_owned();
    let ci_id: Uuid = ci["identity"]["id"]
        .as_str()
        .expect("id")
        .parse()
        .expect("uuid");
    let (s, a) = l.req("POST", "/api/v1/identity/assignments", &w.admin, Some(json!({ "principal": { "type": "service", "id": ci_id }, "role": { "type": "builtin", "key": "metrics-reader" }, "scope": { "type": "platform" } }))).await;
    assert_eq!(s, StatusCode::CREATED, "{a}");
    // It can scrape and read the set-up, and nothing else.
    let (s, _) = l.req("GET", "/api/v1/cluster", &token, None).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "least privilege");
    let (s, info) = l.req("GET", "/api/v1/monitoring", &token, None).await;
    assert_eq!(s, StatusCode::OK, "{info}");
    assert_eq!(info["mayScrape"], true);
    assert_eq!(info["role"], "metrics-reader");
    assert_eq!(info["sector"], "default");
    assert_eq!(info["endpoints"][0]["authentication"], "bearer");
    assert_eq!(
        info["endpoints"].as_array().map(|e| e.len()),
        Some(2),
        "API and worker; no unauthenticated listener unless configured: {info}"
    );
    let (s, _) = l.req("GET", "/api/v1/monitoring", &nobody, None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    // One job through the real worker loop.
    apelora_persistence::enqueue_job(&l.pool, "noop", None, &json!({}), None)
        .await
        .expect("enqueue");
    apelora_persistence::enqueue_job(&l.pool, "noop", None, &json!({}), None)
        .await
        .expect("enqueue");
    let provider = Provider::Mock(MockIncus::default());
    assert!(
        apelora_deployment_worker::run_once(&l.pool, &provider, "metrics-test")
            .await
            .expect("run")
    );
    // A legacy read-only token may scrape too (it already reads the whole inventory).
    let (legacy, _) = apelora_persistence::create_token(&l.pool, "old-collector", "auditor", None)
        .await
        .expect("token");
    let (s, _, _) = get_text(l.state.clone(), "/metrics", Some(&legacy), None).await;
    assert_eq!(s, StatusCode::OK);
    let (s, text, _) = get_text(l.state.clone(), "/metrics", Some(&token), None).await;
    assert_eq!(s, StatusCode::OK);
    for family in [
        "apelora_http_requests_total",
        "apelora_http_request_duration_seconds",
        "apelora_auth_attempts_total",
        "apelora_authz_denials_total",
        "apelora_jobs",
        "apelora_jobs_oldest_runnable_age_seconds",
        "apelora_worker_job_attempts_total",
        "apelora_worker_job_duration_seconds",
        "apelora_db_pool_connections",
        "apelora_db_pool_acquire_seconds",
        "apelora_incus_members",
        "apelora_incus_instances",
        "apelora_applications",
        "apelora_build_info",
        "process_start_time_seconds",
    ] {
        assert!(
            text.contains(&format!("# TYPE {family} ")),
            "{family} missing"
        );
    }
    assert!(
        sample(
            &text,
            "apelora_worker_job_attempts_total",
            &[("kind", "noop"), ("outcome", "succeeded")]
        )
        .is_some_and(|v| v >= 1.0),
        "{text}"
    );
    assert_eq!(
        sample(
            &text,
            "apelora_jobs",
            &[("kind", "noop"), ("status", "pending")]
        ),
        Some(1.0),
        "{text}"
    );
    assert_eq!(sample(&text, "apelora_db_up", &[]), Some(1.0));
    assert!(
        sample(
            &text,
            "apelora_auth_attempts_total",
            &[("method", "service"), ("result", "ok")]
        )
        .is_some_and(|v| v >= 1.0)
    );
    assert!(
        sample(
            &text,
            "apelora_auth_attempts_total",
            &[("method", "none"), ("result", "refused")]
        )
        .is_some_and(|v| v >= 1.0)
    );
    // Nothing secret, personal or identifying.
    for leak in [
        token.as_str(),
        legacy.as_str(),
        "aps_",
        "apl_",
        "Bearer",
        "@example.test",
        "nobody",
        "metrics-collector",
        "old-collector",
        &ci_id.to_string(),
        &w.acme.to_string(),
        "BEGIN",
    ] {
        assert!(!text.contains(leak), "{leak} appears in the metrics");
    }
    // No label value looks like a UUID (36 characters with dashes at 8-13-18-23).
    for value in text.split('"').skip(1).step_by(2) {
        let b = value.as_bytes();
        let uuid_like = b.len() == 36 && [8, 13, 18, 23].iter().all(|i| b[*i] == b'-');
        assert!(!uuid_like, "label value {value} looks like an id");
    }
}
