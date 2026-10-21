// SPDX-License-Identifier: AGPL-3.0-only
//! Request correlation and the access log.
//!
//! Every request gets a request id: the caller's `x-request-id` when it is safe to repeat
//! (at most 64 characters of `A-Z a-z 0-9 . _ -`), otherwise a new UUID. The id is echoed in
//! the response's `x-request-id`, carried by every log line written while the request is
//! handled (span `request` with `request_id`, `method`, `path`), and added as `requestId` to
//! the detail of audit events recorded during the request. The query string is never logged:
//! it can carry values a caller would not want in a log.

use std::sync::Arc;
use std::time::Instant;

use axum::extract::{MatchedPath, Request};
use axum::http::HeaderValue;
use axum::middleware::Next;
use axum::response::Response;
use serde_json::Value;
use tracing::Instrument;

pub const REQUEST_ID_HEADER: &str = "x-request-id";

tokio::task_local! {
    static REQUEST_ID: Arc<str>;
}

/// The id of the request being handled, if any (not inside spawned tasks).
pub fn current() -> Option<Arc<str>> {
    REQUEST_ID.try_with(Arc::clone).ok()
}

/// Adds `requestId` to an audit event's detail when it is recorded during a request.
pub fn tag(detail: &mut Value) {
    if let (Some(id), Some(o)) = (current(), detail.as_object_mut()) {
        o.entry("requestId").or_insert_with(|| Value::from(&*id));
    }
}

/// A caller-supplied id is repeated only when it cannot forge or break a log line.
pub(super) fn acceptable(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// The machine code and message of an error response (see `ApiError`), for the access log and
/// the audit of refusals.
#[derive(Debug, Clone)]
pub struct ErrorCode {
    pub code: &'static str,
    pub message: String,
}

/// Keeps the in-flight gauge right even when a request is cancelled mid-way.
struct InFlight;

impl InFlight {
    fn start() -> Self {
        apelora_metrics::HTTP_IN_FLIGHT.add(&[], 1.0);
        InFlight
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        apelora_metrics::HTTP_IN_FLIGHT.add(&[], -1.0);
    }
}

/// The route template of a request for metrics (`/api/v1/applications/{name}`), never the
/// path itself: a path carries names and ids. Requests that match no route share one label.
fn route_label(req: &Request) -> String {
    req.extensions()
        .get::<MatchedPath>()
        .map_or_else(|| "unmatched".to_owned(), |m| m.as_str().to_owned())
}

/// Outermost layer: assigns the request id, opens the request span and writes one line per
/// completed request (debug for successes, info for 4xx, warn for 5xx). Also counts the request
/// for metrics by route template, method and status class, with its latency.
pub async fn trace(req: Request, next: Next) -> Response {
    let id: Arc<str> = req
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .filter(|v| acceptable(v))
        .map(Arc::from)
        .unwrap_or_else(|| Arc::from(uuid::Uuid::new_v4().to_string()));
    // Error level so the request context reaches every enabled line, even with RUST_LOG=warn.
    let span = tracing::error_span!(
        "request",
        request_id = %id,
        method = %req.method(),
        path = req.uri().path(),
    );
    let started = Instant::now();
    let route = route_label(&req);
    let method = apelora_metrics::method_label(req.method().as_str());
    let in_flight = InFlight::start();
    let mut res = REQUEST_ID
        .scope(id.clone(), next.run(req))
        .instrument(span.clone())
        .await;
    drop(in_flight);
    let status = res.status();
    apelora_metrics::HTTP_REQUESTS.inc(&[
        &route,
        method,
        apelora_metrics::status_class(status.as_u16()),
    ]);
    apelora_metrics::HTTP_DURATION.observe(&[&route, method], started.elapsed().as_secs_f64());
    if status == axum::http::StatusCode::TOO_MANY_REQUESTS {
        apelora_metrics::HTTP_RATE_LIMITED.inc(&[&route]);
    }
    let ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let code = res.extensions().get::<ErrorCode>().map_or("-", |c| c.code);
    span.in_scope(|| {
        if status.is_server_error() {
            tracing::warn!(
                status = status.as_u16(),
                latency_ms = ms,
                code,
                "request failed"
            );
        } else if status.is_client_error() {
            tracing::info!(
                status = status.as_u16(),
                latency_ms = ms,
                code,
                "request refused"
            );
        } else {
            tracing::debug!(
                status = status.as_u16(),
                latency_ms = ms,
                "request completed"
            );
        }
    });
    if let Ok(v) = HeaderValue::from_str(&id) {
        res.headers_mut().insert(REQUEST_ID_HEADER, v);
    }
    res
}
