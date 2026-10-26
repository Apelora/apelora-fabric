// SPDX-License-Identifier: AGPL-3.0-only
//! Apelora deployment worker. Needs `DATABASE_URL`; without it the worker reports that it has
//! nothing to work from and exits. Talks to Incus when `APELORA_INCUS_URL` and the certificate
//! paths are set (the same settings as the control plane), otherwise to the mock provider.
//! Also runs the replica supervisor every `APELORA_SUPERVISE_SECS` (default 30; 0 turns it off);
//! replicas on a member offline for `APELORA_RECOVER_AFTER_SECS` (default 120) are substituted;
//! a stateless replica failing its port check for `APELORA_HEALTH_FAIL_AFTER_SECS` (default 20)
//! is unhealthy, and restarted once unhealthy for `APELORA_HEALTH_RESTART_AFTER_SECS` (60).
//! The supervisor also restarts a failed member's instances on shared storage on another member
//! once that member is fenced (ADR 0013): `APELORA_FENCE_PROBE` (`off` or `ssh`) with
//! `APELORA_FENCE_PROBE_KEY`, `APELORA_FENCE_PROBE_KNOWN_HOSTS`, `APELORA_FENCE_PROBE_USER`,
//! `APELORA_FENCE_PROBE_TARGETS` and `APELORA_FENCE_LOCKSPACE` (see `fencing`).
//! Metrics and a liveness probe are served on `APELORA_WORKER_METRICS_ADDR` (default
//! `127.0.0.1:9471`, `off` turns it off; no authentication, so loopback or a management network).
//! Alerts are evaluated every `APELORA_ALERT_EVALUATE_SECS` (default 30; 0 turns it off) by the
//! worker holding the `alert-evaluator` lease; webhook secrets are read from the
//! `APELORA_ALERT_SECRET_*` variables the channels name (docs/design/alerts.md).

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    apelora_telemetry::init("apelora-deployment-worker");
    let Some(url) = std::env::var("DATABASE_URL").ok().filter(|u| !u.is_empty()) else {
        tracing::warn!("DATABASE_URL is not set; the worker has no job queue to read and exits");
        return Ok(());
    };
    let place = apelora_metrics::Place::from_env(|k| std::env::var(k).ok())?;
    let metrics_addr = apelora_deployment_worker::metrics::listen_addr(|k| std::env::var(k).ok())?;
    let pool = apelora_persistence::connect(&url).await?;
    let worker = std::env::var("APELORA_WORKER_ID")
        .unwrap_or_else(|_| format!("worker-{}", std::process::id()));
    let provider = apelora_incus_adapter::config::provider_from_env(|k| std::env::var(k).ok())?;
    let mirrors = apelora_incus_adapter::config::image_mirrors_from_env(|k| std::env::var(k).ok())?;
    if !mirrors.is_empty() {
        tracing::info!(
            mirrors = mirrors.len(),
            mirrors_only = mirrors.mirrors_only(),
            "images come from this installation's mirrors"
        );
    }
    apelora_incus_adapter::set_image_mirrors(mirrors);
    tracing::info!(%worker, provider = ?provider.mode(), "deployment worker started");
    apelora_metrics::set_place(place);
    apelora_metrics::set_build("worker", env!("CARGO_PKG_VERSION"));
    apelora_metrics::set_provider(if provider.mode() == apelora_domain::ProviderMode::Mock {
        "mock"
    } else {
        "incus"
    });
    let metrics = match metrics_addr {
        Some(at) => {
            if !at.ip().is_loopback() {
                tracing::warn!(%at, "worker metrics are served without authentication on a non-loopback address; keep it on a management network");
            }
            let l = tokio::net::TcpListener::bind(at).await?;
            tracing::info!(%at, "worker metrics listener (GET /metrics, GET /healthz)");
            Some(tokio::spawn(apelora_deployment_worker::metrics::serve(
                l,
                Some(pool.clone()),
            )))
        }
        None => None,
    };
    let served = apelora_deployment_worker::serve(pool, provider, worker, async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await;
    if let Some(m) = metrics {
        m.abort();
    }
    Ok(served?)
}
