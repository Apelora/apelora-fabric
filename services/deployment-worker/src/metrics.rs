// SPDX-License-Identifier: AGPL-3.0-only
//! The worker's metrics listener: `GET /metrics` (text exposition format, OpenMetrics on
//! request) and `GET /healthz` (the process is alive), and nothing else. It binds to
//! `127.0.0.1:9471` unless `APELORA_WORKER_METRICS_ADDR` says otherwise (`off` turns it off).
//! It has no authentication, so it belongs on loopback or a management network; see
//! `docs/architecture/MONITORING.md`.
//!
//! The job loop, the supervisor and the storage schedule record their own families; this module
//! adds the connection pool, sampled when a collector scrapes (at most every 15 s).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use apelora_metrics as m;
use apelora_persistence::Pool;
use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;

const POOL_EVERY: Duration = Duration::from_secs(15);

/// Where to listen: the setting, the default, or `None` when turned off.
pub fn listen_addr(get: impl Fn(&str) -> Option<String>) -> Result<Option<SocketAddr>, String> {
    let v = get("APELORA_WORKER_METRICS_ADDR")
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| m::WORKER_METRICS_DEFAULT_ADDR.to_owned());
    if v == "off" {
        return Ok(None);
    }
    v.parse::<SocketAddr>()
        .map(Some)
        .map_err(|e| format!("APELORA_WORKER_METRICS_ADDR={v:?}: {e}"))
}

#[derive(Clone)]
struct Listener {
    pool: Option<Pool>,
    sampled: Arc<tokio::sync::Mutex<Option<Instant>>>,
}

async fn sample_pool(l: &Listener) {
    let Some(pool) = &l.pool else { return };
    let mut last = l.sampled.lock().await;
    if last.is_some_and(|t| t.elapsed() < POOL_EVERY) {
        return;
    }
    *last = Some(Instant::now());
    let s = apelora_persistence::monitoring::pool_sample(pool, Duration::from_secs(2)).await;
    m::DB_CONNECTIONS.replace([
        (vec!["in_use"], f64::from(s.in_use)),
        (vec!["idle"], f64::from(s.idle)),
        (vec!["max"], f64::from(s.max)),
    ]);
    match s.acquire_secs {
        Some(secs) => {
            m::DB_ACQUIRE.observe(&[], secs);
            m::DB_UP.set(&[], 1.0);
        }
        None => m::DB_UP.set(&[], 0.0),
    }
}

async fn metrics(State(l): State<Listener>, headers: HeaderMap) -> Response {
    sample_pool(&l).await;
    let om = m::wants_openmetrics(headers.get(header::ACCEPT).and_then(|v| v.to_str().ok()));
    (
        StatusCode::OK,
        [
            (
                header::CONTENT_TYPE,
                if om {
                    m::OPENMETRICS_CONTENT_TYPE
                } else {
                    m::TEXT_CONTENT_TYPE
                },
            ),
            (header::CACHE_CONTROL, "no-store"),
        ],
        m::render(om),
    )
        .into_response()
}

/// The listener's routes. `pool` is sampled for the pool families when given.
pub fn router(pool: Option<Pool>) -> Router {
    Router::new()
        .route("/metrics", get(metrics))
        .route("/healthz", get(|| async { "ok\n" }))
        .with_state(Listener {
            pool,
            sampled: Arc::default(),
        })
}

/// Serves the listener until the task is dropped.
pub async fn serve(listener: tokio::net::TcpListener, pool: Option<Pool>) {
    if let Err(e) = axum::serve(listener, router(pool)).await {
        tracing::error!(error = %e, "metrics listener stopped");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_address_defaults_to_loopback_and_can_be_turned_off() {
        let none = |_: &str| None;
        let a = listen_addr(none).expect("default").expect("on");
        assert!(a.ip().is_loopback());
        assert_eq!(a.to_string(), m::WORKER_METRICS_DEFAULT_ADDR);
        let off = |_: &str| Some("off".to_owned());
        assert_eq!(listen_addr(off), Ok(None));
        let bad = |_: &str| Some("0.0.0.0".to_owned());
        assert!(listen_addr(bad).is_err());
    }

    async fn get_text(addr: SocketAddr, path: &str) -> (u16, String) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut s = tokio::net::TcpStream::connect(addr).await.expect("connect");
        s.write_all(
            format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await
        .expect("write");
        let mut out = String::new();
        s.read_to_string(&mut out).await.expect("read");
        let status = out
            .split_whitespace()
            .nth(1)
            .and_then(|c| c.parse().ok())
            .unwrap_or(0);
        (status, out)
    }

    #[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
    async fn the_listener_serves_metrics_and_liveness_only(pool: Pool) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(serve(listener, Some(pool.clone())));
        // A job through the real loop, so the job families have a sample.
        apelora_persistence::enqueue_job(&pool, "noop", None, &serde_json::json!({}), None)
            .await
            .expect("enqueue");
        let provider =
            apelora_incus_adapter::Provider::Mock(apelora_incus_adapter::MockIncus::default());
        assert!(
            crate::run_once(&pool, &provider, "metrics-test")
                .await
                .expect("run")
        );
        // An audit row's message through the relay, so the outbox families have samples.
        apelora_persistence::audit(&pool, "test", "test.event", "t", &serde_json::json!({}))
            .await
            .expect("audit");
        crate::outbox::relay_once(&pool, &crate::outbox::LogSink, None)
            .await
            .expect("relay");
        let (status, body) = get_text(addr, "/metrics").await;
        assert_eq!(status, 200);
        assert!(
            body.contains("apelora_outbox_deliveries_total{region=\"default\",site=\"default\",sector=\"default\",topic=\"audit.recorded\",outcome=\"delivered\"}"),
            "{body}"
        );
        for family in [
            "apelora_outbox_pending{",
            "apelora_outbox_oldest_pending_age_seconds{",
            "apelora_outbox_failing{",
        ] {
            assert!(body.contains(family), "{family}: {body}");
        }
        assert!(
            body.contains("content-type: text/plain; version=0.0.4"),
            "{body}"
        );
        assert!(
            body.contains("apelora_worker_job_attempts_total{region=\"default\",site=\"default\",sector=\"default\",kind=\"noop\",outcome=\"succeeded\"}"),
            "{body}"
        );
        assert!(body.contains("apelora_db_pool_connections{"), "{body}");
        assert!(
            body.contains(
                "apelora_db_up{region=\"default\",site=\"default\",sector=\"default\"} 1"
            ),
            "{body}"
        );
        let (status, body) = get_text(addr, "/healthz").await;
        assert_eq!((status, body.ends_with("ok\n")), (200, true));
        assert_eq!(get_text(addr, "/api/v1/jobs").await.0, 404);
    }
}
