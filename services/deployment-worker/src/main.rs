// SPDX-License-Identifier: AGPL-3.0-only
//! Apelora deployment worker. Needs `DATABASE_URL`; without it the worker reports that it has
//! nothing to work from and exits. Talks to Incus when `APELORA_INCUS_URL` and the certificate
//! paths are set (the same settings as the control plane), otherwise to the mock provider.
//! Also runs the replica supervisor every `APELORA_SUPERVISE_SECS` (default 30; 0 turns it off);
//! replicas on a member offline for `APELORA_RECOVER_AFTER_SECS` (default 120) are substituted;
//! a stateless replica failing its port check for `APELORA_HEALTH_FAIL_AFTER_SECS` (default 20)
//! is unhealthy, and restarted once unhealthy for `APELORA_HEALTH_RESTART_AFTER_SECS` (60).

use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_logging();
    let Some(url) = std::env::var("DATABASE_URL").ok().filter(|u| !u.is_empty()) else {
        tracing::warn!("DATABASE_URL is not set; the worker has no job queue to read and exits");
        return Ok(());
    };
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
    // The replica supervisor restores lost stateless replicas (0 turns it off).
    // A member must stay offline this long before replicas on it are substituted.
    let recover_after: u64 = std::env::var("APELORA_RECOVER_AFTER_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(120);
    let secs = |key: &str, default: f64| {
        std::env::var(key)
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v >= 0.0)
            .unwrap_or(default)
    };
    let health = apelora_domain::health::HealthPolicy {
        fail_after: secs("APELORA_HEALTH_FAIL_AFTER_SECS", 20.0),
        restart_after: secs("APELORA_HEALTH_RESTART_AFTER_SECS", 60.0),
        ..Default::default()
    };
    let every: u64 = std::env::var("APELORA_SUPERVISE_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);
    let supervisor = (every > 0).then(|| {
        tracing::info!(every_secs = every, "replica supervisor started");
        tokio::spawn(apelora_deployment_worker::supervisor::supervise(
            pool.clone(),
            provider.clone(),
            Duration::from_secs(every),
            Duration::from_secs(recover_after),
            health,
        ))
    });
    // Scheduled snapshots and backups of volumes, checked every minute; backups go to the pool
    // in APELORA_BACKUP_POOL (without it, scheduled backups report that none is configured).
    let backup_pool = std::env::var("APELORA_BACKUP_POOL")
        .ok()
        .filter(|p| !p.trim().is_empty());
    if let Some(p) = &backup_pool {
        if !apelora_domain::is_valid_name(p) {
            return Err(format!("APELORA_BACKUP_POOL: invalid pool name {p:?}").into());
        }
        tracing::info!(pool = %p, "volume backups go to this pool");
    }
    let storage = tokio::spawn(apelora_deployment_worker::storage::schedule(
        pool.clone(),
        provider.clone(),
        Duration::from_secs(60),
        backup_pool,
    ));
    apelora_deployment_worker::run(pool, provider, worker, Duration::from_secs(2), async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await;
    if let Some(s) = supervisor {
        s.abort();
    }
    storage.abort();
    Ok(())
}

/// Logging: `RUST_LOG` chooses what is written (default `info`, e.g.
/// `RUST_LOG=info,apelora_deployment_worker=debug`); `APELORA_LOG_FORMAT=json` writes one JSON object per line
/// instead of text. Kept the same in both binaries (`apelora-control-plane`,
/// `apelora-deployment-worker`).
fn init_logging() {
    use tracing_subscriber::EnvFilter;
    let filter = match std::env::var("RUST_LOG") {
        Ok(v) if !v.trim().is_empty() => EnvFilter::try_new(&v).unwrap_or_else(|e| {
            eprintln!("RUST_LOG={v:?} is not a valid filter ({e}); logging at info");
            EnvFilter::new("info")
        }),
        _ => EnvFilter::new("info"),
    };
    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    match std::env::var("APELORA_LOG_FORMAT").as_deref() {
        // The current span (request or job) only: one copy of its fields per line.
        Ok("json") => builder.json().with_span_list(false).init(),
        Ok("" | "text") | Err(_) => builder.init(),
        Ok(other) => {
            builder.init();
            tracing::warn!(
                format = other,
                "APELORA_LOG_FORMAT must be text or json; writing text"
            );
        }
    }
}
