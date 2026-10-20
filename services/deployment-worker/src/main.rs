// SPDX-License-Identifier: AGPL-3.0-only
//! Apelora deployment worker. Needs `DATABASE_URL`; without it the worker reports that it has
//! nothing to work from and exits. Talks to Incus when `APELORA_INCUS_URL` and the certificate
//! paths are set (the same settings as the control plane), otherwise to the mock provider.

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
    apelora_deployment_worker::run(pool, provider, worker, Duration::from_secs(2), async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await;
    Ok(())
}
