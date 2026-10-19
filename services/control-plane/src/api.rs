// SPDX-License-Identifier: AGPL-3.0-only
//! `/api/v1` routes. Every inventory response carries its source (mock or Incus) and the time
//! it was observed, so the UI can never mistake mock data for a real cluster.

use std::sync::Arc;

use apelora_domain::{API_VERSION, ProviderMode};
use apelora_incus_adapter::{AdapterError, Provider};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{Value, json};

#[derive(Clone)]
pub struct AppState {
    provider: Arc<Provider>,
}

impl AppState {
    pub fn new(provider: Provider) -> Self {
        Self {
            provider: Arc::new(provider),
        }
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/v1/health", get(health))
        .route("/api/v1/cluster", get(cluster))
        .route("/api/v1/instances", get(instances))
        .route("/api/v1/projects", get(projects))
        .route("/api/v1/networks", get(networks))
        .route("/api/v1/storage-pools", get(storage_pools))
        .with_state(state)
}

fn source(mode: ProviderMode) -> Value {
    json!({ "provider": mode, "observedAt": jiff::Timestamp::now().to_string() })
}

/// Adapter failures become 502/503 responses with a stable code and a readable message.
struct ApiError(AdapterError);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code) = match &self.0 {
            AdapterError::NotConfigured => {
                (StatusCode::SERVICE_UNAVAILABLE, "incus_not_configured")
            }
            AdapterError::InvalidConfig(_) => {
                (StatusCode::SERVICE_UNAVAILABLE, "incus_invalid_config")
            }
            AdapterError::Connectivity(_) => (StatusCode::BAD_GATEWAY, "incus_unreachable"),
            AdapterError::Authentication(_) => (StatusCode::BAD_GATEWAY, "incus_authentication"),
            AdapterError::NotFound(_) => (StatusCode::BAD_GATEWAY, "incus_not_found"),
            AdapterError::Api { .. } => (StatusCode::BAD_GATEWAY, "incus_api_error"),
            AdapterError::Decode(_) => (StatusCode::BAD_GATEWAY, "incus_bad_response"),
            AdapterError::UnsupportedCapability(_) => {
                (StatusCode::BAD_GATEWAY, "incus_unsupported_capability")
            }
        };
        tracing::warn!(code, error = %self.0, "inventory request failed");
        (
            status,
            Json(json!({ "error": { "code": code, "message": self.0.to_string() } })),
        )
            .into_response()
    }
}

impl From<AdapterError> for ApiError {
    fn from(e: AdapterError) -> Self {
        Self(e)
    }
}

async fn health(State(state): State<AppState>) -> Json<Value> {
    let mode = state.provider.mode();
    let incus = match state.provider.server_info().await {
        Ok(info) => {
            json!({ "connected": mode == ProviderMode::Incus, "apiVersion": info.api_version, "serverVersion": info.server_version, "clustered": info.clustered })
        }
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

async fn cluster(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let p = &state.provider;
    let server = p.server_info().await?;
    let members = p.cluster_members().await?;
    Ok(Json(
        json!({ "source": source(p.mode()), "server": server, "members": members }),
    ))
}

async fn instances(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let items = state.provider.instances().await?;
    Ok(Json(
        json!({ "source": source(state.provider.mode()), "items": items }),
    ))
}

async fn projects(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let items = state.provider.projects().await?;
    Ok(Json(
        json!({ "source": source(state.provider.mode()), "items": items }),
    ))
}

async fn networks(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let items = state.provider.networks().await?;
    Ok(Json(
        json!({ "source": source(state.provider.mode()), "items": items }),
    ))
}

async fn storage_pools(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let items = state.provider.storage_pools().await?;
    Ok(Json(
        json!({ "source": source(state.provider.mode()), "items": items }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use apelora_incus_adapter::{IncusClient, MockIncus};
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn call(provider: Provider, path: &str) -> (StatusCode, Value) {
        let res = router(AppState::new(provider))
            .oneshot(Request::get(path).body(Body::empty()).expect("request"))
            .await
            .expect("response");
        let status = res.status();
        let bytes = res.into_body().collect().await.expect("body").to_bytes();
        (status, serde_json::from_slice(&bytes).expect("json"))
    }

    #[tokio::test]
    async fn health_reports_mock_provider_and_no_incus_connection() {
        let (status, body) = call(Provider::Mock(MockIncus::default()), "/api/v1/health").await;
        assert_eq!(status, 200);
        assert_eq!(body["provider"], "mock");
        assert_eq!(body["incus"]["connected"], false);
        assert_eq!(body["incus"]["serverVersion"], Value::Null);
    }

    #[tokio::test]
    async fn inventory_endpoints_label_their_source() {
        for path in [
            "/api/v1/cluster",
            "/api/v1/instances",
            "/api/v1/projects",
            "/api/v1/networks",
            "/api/v1/storage-pools",
        ] {
            let (status, body) = call(Provider::Mock(MockIncus::default()), path).await;
            assert_eq!(status, 200, "{path}");
            assert_eq!(body["source"]["provider"], "mock", "{path}");
            assert!(
                body["source"]["observedAt"]
                    .as_str()
                    .is_some_and(|t| t.ends_with('Z')),
                "{path}: {body}"
            );
        }
        let (_, cluster) = call(Provider::Mock(MockIncus::default()), "/api/v1/cluster").await;
        assert_eq!(cluster["members"].as_array().map(Vec::len), Some(3));
        assert_eq!(cluster["members"][0]["failureDomain"], "default");
        let (_, inst) = call(Provider::Mock(MockIncus::default()), "/api/v1/instances").await;
        assert_eq!(inst["items"][3]["kind"], "virtual-machine");
    }

    #[tokio::test]
    async fn unreachable_incus_is_a_502_with_a_stable_code() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        drop(listener);
        let provider = Provider::Incus(IncusClient::plain_for_tests(&format!("http://{addr}")));
        let (status, body) = call(provider.clone(), "/api/v1/instances").await;
        assert_eq!(status, 502);
        assert_eq!(body["error"]["code"], "incus_unreachable");
        // Health stays 200 and says it is not connected, so the console can explain why.
        let (status, body) = call(provider, "/api/v1/health").await;
        assert_eq!(status, 200);
        assert_eq!(body["incus"]["connected"], false);
    }
}
