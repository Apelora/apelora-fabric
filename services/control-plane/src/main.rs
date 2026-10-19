// SPDX-License-Identifier: AGPL-3.0-only
//! Apelora control plane (Axum REST API).
//!
//! Phase 01 skeleton: one health endpoint, backed by the mock Incus provider. It never
//! claims a connection to a real cluster.

use std::{net::SocketAddr, sync::Arc};

use apelora_domain::{API_VERSION, ProviderMode};
use apelora_incus_adapter::{IncusProvider, MockIncus};
use axum::{Json, Router, extract::State, routing::get};
use serde_json::{Value, json};

#[derive(Clone)]
struct AppState {
    provider: Arc<dyn IncusProvider>,
}

fn app(state: AppState) -> Router {
    Router::new()
        .route("/api/v1/health", get(health))
        .with_state(state)
}

async fn health(State(state): State<AppState>) -> Json<Value> {
    let mode = state.provider.mode();
    let incus = match state.provider.server_info() {
        Ok(info) => json!({
            "connected": mode == ProviderMode::Incus,
            "apiVersion": info.api_version,
            "serverVersion": info.server_version,
        }),
        Err(err) => json!({ "connected": false, "error": err.to_string() }),
    };
    Json(json!({
        "status": "ok",
        "service": "control-plane",
        "version": env!("CARGO_PKG_VERSION"),
        "apiVersion": API_VERSION,
        "provider": mode,
        "incus": incus,
    }))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().init();
    // Loopback by default; exposing the API is a deliberate configuration change.
    let addr: SocketAddr = std::env::var("APELORA_API_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:8080".to_owned())
        .parse()?;
    let state = AppState {
        provider: Arc::new(MockIncus::default()),
    };
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, provider = "mock", "control plane listening; not connected to Incus");
    axum::serve(listener, app(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    #[tokio::test]
    async fn health_reports_mock_provider_and_no_incus_connection() {
        let state = AppState {
            provider: Arc::new(MockIncus::default()),
        };
        let res = app(state)
            .oneshot(
                Request::get("/api/v1/health")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(res.status(), 200);
        let bytes = res.into_body().collect().await.expect("body").to_bytes();
        let body: Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(body["status"], "ok");
        assert_eq!(body["provider"], "mock");
        assert_eq!(body["incus"]["connected"], false);
        assert_eq!(body["incus"]["serverVersion"], Value::Null);
    }
}
