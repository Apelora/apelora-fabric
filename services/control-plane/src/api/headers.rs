// SPDX-License-Identifier: AGPL-3.0-only
//! Security headers on every API response.
//!
//! The API answers JSON (and one websocket upgrade); nothing it returns is meant to be rendered,
//! framed or cached by a browser or a shared cache. Every response therefore says so:
//!
//! - `X-Content-Type-Options: nosniff` — a browser never guesses another type than the one sent;
//! - `Referrer-Policy: no-referrer`;
//! - `X-Frame-Options: DENY` and `Content-Security-Policy: default-src 'none'; frame-ancestors 'none'`
//!   — a response opened directly in a browser runs nothing and cannot be framed;
//! - `Cache-Control: no-store` — answers carry data for one caller (authenticated) or a status that
//!   changes (public); a handler that sets its own `Cache-Control` keeps it;
//! - `Strict-Transport-Security` only when this process serves TLS itself (`APELORA_TLS_CERT`).
//!   Behind a TLS-terminating proxy, the proxy sends it (`infra/serve/`).
//!
//! The web applications are static files served next to the API with their own, wider policy
//! (`infra/serve/`); these headers apply to `/api/v1` only.

use axum::extract::{Request, State};
use axum::http::{HeaderName, HeaderValue, header};
use axum::middleware::Next;
use axum::response::Response;

/// The policy of API responses: nothing may load, and nothing may frame them.
pub const API_CSP: &str = "default-src 'none'; frame-ancestors 'none'";
/// One year; no `includeSubDomains`, since other services may share the parent domain.
pub const HSTS: &str = "max-age=31536000";

/// What differs between installations: whether this process terminates TLS.
#[derive(Debug, Clone, Copy, Default)]
pub struct HeaderPolicy {
    /// Send `Strict-Transport-Security` (only when serving TLS ourselves).
    pub hsts: bool,
}

/// Adds the headers to every response, including errors, refusals and unknown routes.
pub async fn apply(State(policy): State<HeaderPolicy>, req: Request, next: Next) -> Response {
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    let fixed: [(HeaderName, &'static str); 4] = [
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        (header::REFERRER_POLICY, "no-referrer"),
        (header::X_FRAME_OPTIONS, "DENY"),
        (header::CONTENT_SECURITY_POLICY, API_CSP),
    ];
    for (name, value) in fixed {
        h.insert(name, HeaderValue::from_static(value));
    }
    if !h.contains_key(header::CACHE_CONTROL) {
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    if policy.hsts {
        h.insert(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static(HSTS),
        );
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::http::StatusCode;
    use axum::middleware;
    use axum::routing::get;
    use tower::ServiceExt;

    fn app(hsts: bool) -> Router {
        Router::new()
            .route("/plain", get(|| async { "{}" }))
            .route(
                "/own-cache",
                get(|| async { ([(header::CACHE_CONTROL, "private, max-age=5")], "{}") }),
            )
            .layer(middleware::from_fn_with_state(HeaderPolicy { hsts }, apply))
    }

    async fn headers(app: Router, path: &str) -> (StatusCode, axum::http::HeaderMap) {
        let res = app
            .oneshot(Request::get(path).body(Body::empty()).expect("request"))
            .await
            .expect("response");
        (res.status(), res.headers().clone())
    }

    #[tokio::test]
    async fn every_response_says_do_not_sniff_frame_refer_or_store() {
        // Unknown routes (404) get them too: the layer wraps the fallback.
        for path in ["/plain", "/missing"] {
            let (_, h) = headers(app(false), path).await;
            assert_eq!(h["x-content-type-options"], "nosniff", "{path}");
            assert_eq!(h["referrer-policy"], "no-referrer", "{path}");
            assert_eq!(h["x-frame-options"], "DENY", "{path}");
            assert_eq!(h["content-security-policy"], API_CSP, "{path}");
            assert_eq!(h["cache-control"], "no-store", "{path}");
            assert!(!h.contains_key("strict-transport-security"), "{path}");
        }
    }

    #[tokio::test]
    async fn a_handler_keeps_its_own_cache_rule_and_tls_adds_hsts() {
        let (_, h) = headers(app(true), "/own-cache").await;
        assert_eq!(h["cache-control"], "private, max-age=5");
        assert_eq!(h["strict-transport-security"], HSTS);
    }
}
