// SPDX-License-Identifier: AGPL-3.0-only
//! Apelora deployment worker. Needs `DATABASE_URL`; without it the worker reports that it has
//! nothing to work from and exits. Talks to Incus when `APELORA_INCUS_URL` and the certificate
//! paths are set (the same settings as the control plane), otherwise to the mock provider.
//! Also runs the replica supervisor every `APELORA_SUPERVISE_SECS` (default 30; 0 turns it off).

use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().init();
    let Some(url) = std::env::var("DATABASE_URL").ok().filter(|u| !u.is_empty()) else {
        tracing::warn!("DATABASE_URL is not set; the worker has no job queue to read and exits");
        return Ok(());
    };
    let pool = apelora_persistence::connect(&url).await?;
    let worker = std::env::var("APELORA_WORKER_ID")
        .unwrap_or_else(|_| format!("worker-{}", std::process::id()));
    let provider = apelora_incus_adapter::config::provider_from_env(|k| std::env::var(k).ok())?;
    tracing::info!(%worker, provider = ?provider.mode(), "deployment worker started");
    // The replica supervisor restores lost stateless replicas (0 turns it off).
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
        ))
    });
    apelora_deployment_worker::run(pool, provider, worker, Duration::from_secs(2), async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await;
    if let Some(s) = supervisor {
        s.abort();
    }
    Ok(())
}
