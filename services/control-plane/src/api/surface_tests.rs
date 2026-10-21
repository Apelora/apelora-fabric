// SPDX-License-Identifier: AGPL-3.0-only
//! The public surface through the whole router: security headers, the redacted and cached
//! `/health`, and the rate limits (per address, per refused credential, per caller's writes).

use super::*;
use apelora_incus_adapter::{IncusClient, MockIncus};
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, Request};
use http_body_util::BodyExt;
use serde_json::json;
use std::net::SocketAddr;
use tower::ServiceExt;

/// Sends a request as if it came from `peer` over a real connection.
async fn send(
    state: &AppState,
    method: &str,
    path: &str,
    peer: &str,
    headers: &[(&str, &str)],
) -> (StatusCode, HeaderMap, Value) {
    let mut req = Request::builder().method(method).uri(path);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let mut req = req.body(Body::empty()).expect("request");
    let peer: SocketAddr = peer.parse().expect("peer");
    req.extensions_mut().insert(ConnectInfo(peer));
    let res = router(state.clone()).oneshot(req).await.expect("response");
    let (status, headers) = (res.status(), res.headers().clone());
    let bytes = res.into_body().collect().await.expect("body").to_bytes();
    (
        status,
        headers,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn dev() -> AppState {
    AppState::new(
        Provider::Mock(MockIncus::default()),
        Auth::DisabledForDevelopment,
    )
}

fn unreachable_incus() -> Provider {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    drop(listener);
    Provider::Incus(IncusClient::plain_for_tests(&format!("http://{addr}")))
}

fn limits(
    public: (u32, u32),
    failed: (u32, u32),
    writes: (u32, u32),
) -> rate_limit::RateLimitConfig {
    let r = |(per_minute, burst)| rate_limit::Rate { per_minute, burst };
    rate_limit::RateLimitConfig {
        public: r(public),
        failed_auth: r(failed),
        writes: r(writes),
        ..Default::default()
    }
}

#[tokio::test]
async fn api_responses_carry_the_security_headers_errors_included() {
    let state = dev();
    for (method, path) in [
        ("GET", "/api/v1/health"),
        ("GET", "/api/v1/instances"),
        ("GET", "/api/v1/no-such-route"),
        ("POST", "/api/v1/instances"),
    ] {
        let (_, h, _) = send(&state, method, path, "192.0.2.1:5000", &[]).await;
        assert_eq!(h["x-content-type-options"], "nosniff", "{path}");
        assert_eq!(h["referrer-policy"], "no-referrer", "{path}");
        assert_eq!(h["x-frame-options"], "DENY", "{path}");
        assert_eq!(
            h["content-security-policy"], "default-src 'none'; frame-ancestors 'none'",
            "{path}"
        );
        assert_eq!(h["cache-control"], "no-store", "{path}");
        assert!(h.get("x-request-id").is_some(), "{path}");
        assert!(
            !h.contains_key("strict-transport-security"),
            "plain HTTP: {path}"
        );
    }
    let (_, h, _) = send(
        &dev().with_tls(true),
        "GET",
        "/api/v1/health",
        "192.0.2.1:5000",
        &[],
    )
    .await;
    assert_eq!(h["strict-transport-security"], "max-age=31536000");
}

#[tokio::test]
async fn public_health_names_no_incus_versions_or_errors_and_system_does_for_signed_in_callers() {
    let state = AppState::new(unreachable_incus(), Auth::DisabledForDevelopment);
    let (status, _, body) = send(&state, "GET", "/api/v1/health", "192.0.2.1:5000", &[]).await;
    assert_eq!(status, 200);
    let keys: Vec<&str> = body
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "apiVersion",
            "auth",
            "checkedAt",
            "incus",
            "provider",
            "service",
            "status",
            "version"
        ]
    );
    assert_eq!(body["incus"], json!({ "connected": false }));
    let text = body.to_string();
    for leak in [
        "error",
        "Version\":\"7",
        "127.0.0.1",
        "refused",
        "serverVersion",
    ] {
        assert!(!text.contains(leak), "{leak} in {text}");
    }
    // The detail is on the authenticated system route.
    let (status, _, system) = send(&state, "GET", "/api/v1/system", "192.0.2.1:5000", &[]).await;
    assert_eq!(status, 200);
    assert_eq!(system["incus"]["connected"], false);
    assert!(
        system["incus"]["error"]
            .as_str()
            .is_some_and(|e| !e.is_empty()),
        "{system}"
    );
    let (_, _, mock) = send(&dev(), "GET", "/api/v1/system", "192.0.2.1:5000", &[]).await;
    assert_eq!(mock["incus"]["apiVersion"], "1.0", "{mock}");
}

#[tokio::test]
async fn public_routes_are_limited_per_address_with_retry_after() {
    let state = dev().with_rate_limits(limits((60, 3), (10, 5), (600, 200)));
    for _ in 0..3 {
        let (status, _, _) = send(&state, "GET", "/api/v1/health", "198.51.100.1:4000", &[]).await;
        assert_eq!(status, 200);
    }
    let (status, h, body) = send(&state, "GET", "/api/v1/sign-in", "198.51.100.1:4001", &[]).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body["error"]["code"], "rate_limited");
    assert_eq!(h["retry-after"], "1");
    assert_eq!(
        h["x-content-type-options"], "nosniff",
        "refusals carry the headers too"
    );
    // A bearer header does not make a public route private.
    let (status, _, _) = send(
        &state,
        "GET",
        "/api/v1/health",
        "198.51.100.1:4002",
        &[("authorization", "Bearer made-up")],
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    // Another address is not affected; a forwarded-for header from an untrusted peer is ignored.
    let (status, _, _) = send(&state, "GET", "/api/v1/health", "198.51.100.2:4000", &[]).await;
    assert_eq!(status, 200);
    let (status, _, _) = send(
        &state,
        "GET",
        "/api/v1/health",
        "198.51.100.1:4003",
        &[("x-forwarded-for", "203.0.113.50")],
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    // In development mode (no sign-in), protected routes are not counted as public.
    for _ in 0..10 {
        let (status, _, _) = send(&state, "GET", "/api/v1/whoami", "198.51.100.1:4004", &[]).await;
        assert_eq!(status, 200);
    }
}

#[tokio::test]
async fn behind_a_trusted_proxy_each_forwarded_client_has_its_own_bucket() {
    let mut cfg = limits((60, 2), (10, 5), (600, 200));
    cfg.trusted_proxies = vec![rate_limit::Cidr::parse("10.0.0.0/24").expect("cidr")];
    let state = dev().with_rate_limits(cfg);
    let via_proxy = |client: &'static str| [("x-forwarded-for", client)];
    for _ in 0..2 {
        let (s, _, _) = send(
            &state,
            "GET",
            "/api/v1/health",
            "10.0.0.2:4000",
            &via_proxy("203.0.113.1"),
        )
        .await;
        assert_eq!(s, 200);
    }
    let (s, _, _) = send(
        &state,
        "GET",
        "/api/v1/health",
        "10.0.0.2:4000",
        &via_proxy("203.0.113.1"),
    )
    .await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
    let (s, _, _) = send(
        &state,
        "GET",
        "/api/v1/health",
        "10.0.0.2:4000",
        &via_proxy("203.0.113.2"),
    )
    .await;
    assert_eq!(s, 200, "another client behind the same proxy");
}

#[tokio::test]
async fn writes_are_limited_per_caller_and_reads_are_not() {
    let state = dev().with_rate_limits(limits((600, 200), (10, 5), (60, 2)));
    for _ in 0..2 {
        let (s, _, _) = send(
            &state,
            "POST",
            "/api/v1/instances",
            "192.0.2.9:1",
            &[("content-type", "application/json")],
        )
        .await;
        assert_ne!(s, StatusCode::TOO_MANY_REQUESTS);
    }
    let (s, h, body) = send(
        &state,
        "POST",
        "/api/v1/instances",
        "192.0.2.10:1",
        &[("content-type", "application/json")],
    )
    .await;
    assert_eq!(
        s,
        StatusCode::TOO_MANY_REQUESTS,
        "the same caller from another address"
    );
    assert_eq!(body["error"]["code"], "rate_limited");
    assert!(h.contains_key("retry-after"));
    for _ in 0..5 {
        let (s, _, _) = send(&state, "GET", "/api/v1/instances", "192.0.2.9:1", &[]).await;
        assert_eq!(s, 200);
    }
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn refused_credentials_lock_out_further_guesses_from_that_address(pool: Pool) {
    let state = AppState::new(
        Provider::Mock(MockIncus::default()),
        Auth::Tokens(pool.clone()),
    )
    .with_rate_limits(limits((600, 200), (10, 3), (600, 200)));
    let (good, _) = apelora_persistence::create_token(&pool, "ops", "auditor", None)
        .await
        .expect("token");
    let guess = [("authorization", "Bearer apl_0000")];
    for _ in 0..3 {
        let (s, _, _) = send(&state, "GET", "/api/v1/whoami", "203.0.113.7:1", &guess).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
    }
    let (s, h, _) = send(&state, "GET", "/api/v1/whoami", "203.0.113.7:2", &guess).await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
    assert!(h.contains_key("retry-after"));
    // The lock-out is per address: a valid token elsewhere works, and successes cost nothing.
    let bearer = format!("Bearer {good}");
    for _ in 0..5 {
        let (s, _, _) = send(
            &state,
            "GET",
            "/api/v1/whoami",
            "203.0.113.8:1",
            &[("authorization", &bearer)],
        )
        .await;
        assert_eq!(s, 200);
    }
    // Requests without any token are counted as public when sign-in is required.
    let state = AppState::new(Provider::Mock(MockIncus::default()), Auth::Tokens(pool))
        .with_rate_limits(limits((60, 2), (10, 3), (600, 200)));
    for _ in 0..2 {
        let (s, _, _) = send(&state, "GET", "/api/v1/instances", "203.0.113.9:1", &[]).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
    }
    let (s, _, _) = send(&state, "GET", "/api/v1/instances", "203.0.113.9:1", &[]).await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
}

/// A route that needs no sign-in must be counted by the public limit. Fails when a new public
/// route is added without listing it in `rate_limit::PUBLIC_PATHS`.
#[test]
fn every_route_without_authentication_is_rate_limited_as_public() {
    let doc: Value = serde_json::from_str(&openapi_json()).expect("openapi");
    let mut public = 0;
    for (path, item) in doc["paths"].as_object().expect("paths") {
        for (method, op) in item.as_object().expect("item") {
            if op["security"] == json!([{}]) {
                public += 1;
                assert!(
                    rate_limit::is_public_path(path),
                    "{} {path} needs no authentication but is not in rate_limit::PUBLIC_PATHS",
                    method.to_uppercase()
                );
            }
        }
    }
    assert!(public >= 2, "{public}");
}
