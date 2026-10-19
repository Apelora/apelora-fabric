// SPDX-License-Identifier: AGPL-3.0-only
//! Apelora control plane (Axum REST API, `/api/v1`).
//!
//! Read-only inventory from Incus through the adapter. With no Incus settings it serves
//! clearly labelled mock data; with settings it talks to Incus over mutual TLS and refuses to
//! start if they are incomplete (it never falls back to mock data silently).

mod api;
mod config;

use std::net::SocketAddr;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().init();
    // Loopback by default; exposing the API is a deliberate configuration change.
    let addr: SocketAddr = std::env::var("APELORA_API_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:8080".to_owned())
        .parse()?;
    let provider = config::provider_from_env(|k| std::env::var(k).ok())?;
    let mode = provider.mode();
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, provider = ?mode, "control plane listening");
    axum::serve(listener, api::router(api::AppState::new(provider)))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
