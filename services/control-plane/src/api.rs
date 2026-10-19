// SPDX-License-Identifier: AGPL-3.0-only
//! `/api/v1` routes. Every route except `/health` needs an authenticated principal and names
//! the permission it requires. Inventory responses carry their source (mock or Incus) and the
//! time they were observed, so mock data is never mistaken for a real cluster.

use std::sync::Arc;

use apelora_domain::{API_VERSION, Permission, ProviderMode, Role};
use apelora_incus_adapter::{AdapterError, Provider};
use apelora_persistence::Pool;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router};
use serde_json::{Value, json};

/// How requests are authenticated.
#[derive(Clone)]
pub enum Auth {
    /// Bearer API tokens checked against the database.
    Tokens(Pool),
    /// Development only (loopback): every request acts as a fabric admin, and `/health` says so.
    DisabledForDevelopment,
}

#[derive(Clone)]
pub struct AppState {
    provider: Arc<Provider>,
    auth: Auth,
}

impl AppState {
    pub fn new(provider: Provider, auth: Auth) -> Self {
        Self {
            provider: Arc::new(provider),
            auth,
        }
    }
}

/// The caller of a request.
#[derive(Debug, Clone)]
pub struct Principal {
    pub name: String,
    pub role: Role,
}

pub fn router(state: AppState) -> Router {
    let protected = Router::new()
        .route("/api/v1/whoami", get(whoami))
        .route("/api/v1/cluster", get(cluster))
        .route("/api/v1/instances", get(instances))
        .route("/api/v1/projects", get(projects))
        .route("/api/v1/networks", get(networks))
        .route("/api/v1/storage-pools", get(storage_pools))
        .route("/api/v1/audit", get(audit))
        .route_layer(middleware::from_fn_with_state(state.clone(), authenticate));
    Router::new()
        .route("/api/v1/health", get(health))
        .merge(protected)
        .with_state(state)
}

/// API errors with a stable machine code and a readable message.
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ApiError {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        if self.status.is_server_error() || self.status == StatusCode::BAD_GATEWAY {
            tracing::warn!(code = self.code, error = %self.message, "request failed");
        }
        let mut res = (
            self.status,
            Json(json!({ "error": { "code": self.code, "message": self.message } })),
        )
            .into_response();
        if self.status == StatusCode::UNAUTHORIZED {
            res.headers_mut()
                .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        }
        res
    }
}

impl From<AdapterError> for ApiError {
    fn from(e: AdapterError) -> Self {
        let (status, code) = match &e {
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
        ApiError::new(status, code, e.to_string())
    }
}

impl From<apelora_persistence::StoreError> for ApiError {
    fn from(e: apelora_persistence::StoreError) -> Self {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "store_unavailable",
            e.to_string(),
        )
    }
}

fn require(p: &Principal, perm: Permission) -> Result<(), ApiError> {
    if p.role.allows(perm) {
        Ok(())
    } else {
        Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            format!("role {} may not {perm:?}", p.role.as_str()),
        ))
    }
}

async fn authenticate(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    let principal = match &state.auth {
        Auth::DisabledForDevelopment => Principal {
            name: "development".into(),
            role: Role::FabricAdmin,
        },
        Auth::Tokens(pool) => {
            let presented = req
                .headers()
                .get(header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Bearer "))
                .map(str::trim);
            let Some(token) = presented else {
                return ApiError::new(
                    StatusCode::UNAUTHORIZED,
                    "unauthenticated",
                    "an API token is required (Authorization: Bearer <token>)",
                )
                .into_response();
            };
            match apelora_persistence::authenticate(pool, token).await {
                Ok(Some(rec)) => match Role::parse(&rec.role) {
                    Some(role) => Principal {
                        name: rec.name,
                        role,
                    },
                    None => {
                        return ApiError::new(
                            StatusCode::FORBIDDEN,
                            "forbidden",
                            "token has an unknown role",
                        )
                        .into_response();
                    }
                },
                Ok(None) => {
                    return ApiError::new(
                        StatusCode::UNAUTHORIZED,
                        "unauthenticated",
                        "the API token is unknown, revoked or expired",
                    )
                    .into_response();
                }
                Err(e) => return ApiError::from(e).into_response(),
            }
        }
    };
    req.extensions_mut().insert(principal);
    next.run(req).await
}

fn source(mode: ProviderMode) -> Value {
    json!({ "provider": mode, "observedAt": jiff::Timestamp::now().to_string() })
}

async fn health(State(state): State<AppState>) -> Json<Value> {
    let mode = state.provider.mode();
    let incus = match state.provider.server_info().await {
        Ok(info) => {
            json!({ "connected": mode == ProviderMode::Incus, "apiVersion": info.api_version, "serverVersion": info.server_version, "clustered": info.clustered })
        }
        Err(err) => json!({ "connected": false, "error": err.to_string() }),
    };
    let auth = match state.auth {
        Auth::Tokens(_) => "tokens",
        Auth::DisabledForDevelopment => "disabled-development",
    };
    Json(json!({
        "status": "ok",
        "service": "control-plane",
        "version": env!("CARGO_PKG_VERSION"),
        "apiVersion": API_VERSION,
        "provider": mode,
        "auth": auth,
        "incus": incus,
    }))
}

async fn whoami(Extension(p): Extension<Principal>) -> Json<Value> {
    let perms: Vec<_> = [
        Permission::ReadInventory,
        Permission::ReadAudit,
        Permission::ManageTokens,
        Permission::ManageInfrastructure,
        Permission::DesignApplications,
        Permission::Deploy,
    ]
    .into_iter()
    .filter(|x| p.role.allows(*x))
    .collect();
    Json(json!({ "name": p.name, "role": p.role, "permissions": perms }))
}

async fn cluster(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Json<Value>, ApiError> {
    require(&p, Permission::ReadInventory)?;
    let server = state.provider.server_info().await?;
    let members = state.provider.cluster_members().await?;
    Ok(Json(
        json!({ "source": source(state.provider.mode()), "server": server, "members": members }),
    ))
}

async fn instances(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Json<Value>, ApiError> {
    require(&p, Permission::ReadInventory)?;
    let items = state.provider.instances().await?;
    Ok(Json(
        json!({ "source": source(state.provider.mode()), "items": items }),
    ))
}

async fn projects(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Json<Value>, ApiError> {
    require(&p, Permission::ReadInventory)?;
    let items = state.provider.projects().await?;
    Ok(Json(
        json!({ "source": source(state.provider.mode()), "items": items }),
    ))
}

async fn networks(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Json<Value>, ApiError> {
    require(&p, Permission::ReadInventory)?;
    let items = state.provider.networks().await?;
    Ok(Json(
        json!({ "source": source(state.provider.mode()), "items": items }),
    ))
}

async fn storage_pools(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Json<Value>, ApiError> {
    require(&p, Permission::ReadInventory)?;
    let items = state.provider.storage_pools().await?;
    Ok(Json(
        json!({ "source": source(state.provider.mode()), "items": items }),
    ))
}

async fn audit(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Json<Value>, ApiError> {
    require(&p, Permission::ReadAudit)?;
    let Auth::Tokens(pool) = &state.auth else {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "store_unavailable",
            "the audit log needs a database (DATABASE_URL)",
        ));
    };
    Ok(Json(
        json!({ "items": apelora_persistence::recent_audit(pool, 100).await? }),
    ))
}

#[cfg(test)]
mod tests;
