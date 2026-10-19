// SPDX-License-Identifier: AGPL-3.0-only
//! Apelora deployment worker. Needs `DATABASE_URL`; without it the worker reports that it has
//! nothing to work from and exits. Read-only: no job handler changes Incus yet.

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
    tracing::info!(%worker, read_only = true, "deployment worker started");
    apelora_deployment_worker::run(pool, worker, Duration::from_secs(2), async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await;
    Ok(())
}
