// SPDX-License-Identifier: AGPL-3.0-only
//! `/api/v1` routes. Every route except `/health` needs an authenticated principal and names
//! the permission it requires. Inventory responses carry their source (mock or Incus) and the
//! time they were observed, so mock data is never mistaken for a real cluster.

use std::sync::Arc;

use apelora_domain::{
    API_VERSION, ClusterMember, Instance, Network, Permission, Project, ProviderMode, Role,
    ServerInfo, StoragePool,
};
use apelora_incus_adapter::{AdapterError, Provider};
use apelora_persistence::Pool;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router};
use serde::Serialize;
use serde_json::Value;
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi, ToSchema};

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
        .route("/api/v1/openapi.json", get(openapi))
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
            Json(ErrorBody {
                error: ErrorDetail {
                    code: self.code.into(),
                    message: self.message,
                },
            }),
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

/// Where inventory data came from and when it was read.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub provider: ProviderMode,
    /// RFC 3339 timestamp (UTC).
    pub observed_at: String,
}

fn source(mode: ProviderMode) -> Source {
    Source {
        provider: mode,
        observed_at: jiff::Timestamp::now().to_string(),
    }
}

/// Error body for every non-2xx response.
#[derive(Serialize, ToSchema)]
pub struct ErrorBody {
    pub error: ErrorDetail,
}

#[derive(Serialize, ToSchema)]
pub struct ErrorDetail {
    /// Stable machine-readable code, e.g. `incus_unreachable`, `unauthenticated`, `forbidden`.
    pub code: String,
    pub message: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum AuthMode {
    Tokens,
    DisabledDevelopment,
}

/// What the control plane knows about its Incus connection.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct IncusStatus {
    /// True only when talking to a real Incus server (never for mock data).
    pub connected: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub api_version: Option<String>,
    #[schema(required = true)]
    pub server_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub clustered: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub error: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Health {
    pub status: String,
    pub service: String,
    pub version: String,
    pub api_version: String,
    pub provider: ProviderMode,
    pub auth: AuthMode,
    pub incus: IncusStatus,
}

#[derive(Serialize, ToSchema)]
pub struct WhoAmI {
    pub name: String,
    pub role: Role,
    pub permissions: Vec<Permission>,
}

#[derive(Serialize, ToSchema)]
pub struct ClusterResponse {
    pub source: Source,
    pub server: ServerInfo,
    /// Empty when the Incus server is not clustered.
    pub members: Vec<ClusterMember>,
}

macro_rules! list_response {
    ($name:ident, $item:ty) => {
        #[derive(Serialize, ToSchema)]
        pub struct $name {
            pub source: Source,
            pub items: Vec<$item>,
        }
    };
}
list_response!(InstanceList, Instance);
list_response!(ProjectList, Project);
list_response!(NetworkList, Network);
list_response!(StoragePoolList, StoragePool);

#[derive(Serialize, ToSchema)]
pub struct AuditItem {
    pub id: i64,
    /// RFC 3339 timestamp.
    pub at: String,
    pub actor: String,
    pub action: String,
    pub target: String,
    #[schema(value_type = Object)]
    pub detail: Value,
}

#[derive(Serialize, ToSchema)]
pub struct AuditList {
    pub items: Vec<AuditItem>,
}

/// Public. Reports the service, the inventory provider, the authentication mode and whether
/// Incus is reachable.
#[utoipa::path(get, path = "/api/v1/health", tag = "system", security(()),
    responses((status = 200, body = Health)))]
async fn health(State(state): State<AppState>) -> Json<Health> {
    let mode = state.provider.mode();
    let incus = match state.provider.server_info().await {
        Ok(info) => IncusStatus {
            connected: mode == ProviderMode::Incus,
            api_version: Some(info.api_version),
            server_version: info.server_version,
            clustered: Some(info.clustered),
            error: None,
        },
        Err(err) => IncusStatus {
            connected: false,
            api_version: None,
            server_version: None,
            clustered: None,
            error: Some(err.to_string()),
        },
    };
    Json(Health {
        status: "ok".into(),
        service: "control-plane".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        api_version: API_VERSION.into(),
        provider: mode,
        auth: match state.auth {
            Auth::Tokens(_) => AuthMode::Tokens,
            Auth::DisabledForDevelopment => AuthMode::DisabledDevelopment,
        },
        incus,
    })
}

/// The caller's name, role and the permissions that role grants.
#[utoipa::path(get, path = "/api/v1/whoami", tag = "system",
    responses((status = 200, body = WhoAmI), (status = 401, body = ErrorBody)))]
async fn whoami(Extension(p): Extension<Principal>) -> Json<WhoAmI> {
    let permissions = [
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
    Json(WhoAmI {
        name: p.name,
        role: p.role,
        permissions,
    })
}

/// The Incus server and its cluster members. Needs `read-inventory`.
#[utoipa::path(get, path = "/api/v1/cluster", tag = "inventory", responses(
    (status = 200, body = ClusterResponse), (status = 401, body = ErrorBody),
    (status = 403, body = ErrorBody), (status = 502, body = ErrorBody)))]
async fn cluster(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Json<ClusterResponse>, ApiError> {
    require(&p, Permission::ReadInventory)?;
    let server = state.provider.server_info().await?;
    let members = state.provider.cluster_members().await?;
    Ok(Json(ClusterResponse {
        source: source(state.provider.mode()),
        server,
        members,
    }))
}

/// Instances in all projects. Needs `read-inventory`.
#[utoipa::path(get, path = "/api/v1/instances", tag = "inventory", responses(
    (status = 200, body = InstanceList), (status = 401, body = ErrorBody),
    (status = 403, body = ErrorBody), (status = 502, body = ErrorBody)))]
async fn instances(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Json<InstanceList>, ApiError> {
    require(&p, Permission::ReadInventory)?;
    let items = state.provider.instances().await?;
    Ok(Json(InstanceList {
        source: source(state.provider.mode()),
        items,
    }))
}

/// Incus projects. Needs `read-inventory`.
#[utoipa::path(get, path = "/api/v1/projects", tag = "inventory", responses(
    (status = 200, body = ProjectList), (status = 401, body = ErrorBody),
    (status = 403, body = ErrorBody), (status = 502, body = ErrorBody)))]
async fn projects(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Json<ProjectList>, ApiError> {
    require(&p, Permission::ReadInventory)?;
    let items = state.provider.projects().await?;
    Ok(Json(ProjectList {
        source: source(state.provider.mode()),
        items,
    }))
}

/// Networks in all projects. Needs `read-inventory`.
#[utoipa::path(get, path = "/api/v1/networks", tag = "inventory", responses(
    (status = 200, body = NetworkList), (status = 401, body = ErrorBody),
    (status = 403, body = ErrorBody), (status = 502, body = ErrorBody)))]
async fn networks(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Json<NetworkList>, ApiError> {
    require(&p, Permission::ReadInventory)?;
    let items = state.provider.networks().await?;
    Ok(Json(NetworkList {
        source: source(state.provider.mode()),
        items,
    }))
}

/// Storage pools. Needs `read-inventory`.
#[utoipa::path(get, path = "/api/v1/storage-pools", tag = "inventory", responses(
    (status = 200, body = StoragePoolList), (status = 401, body = ErrorBody),
    (status = 403, body = ErrorBody), (status = 502, body = ErrorBody)))]
async fn storage_pools(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Json<StoragePoolList>, ApiError> {
    require(&p, Permission::ReadInventory)?;
    let items = state.provider.storage_pools().await?;
    Ok(Json(StoragePoolList {
        source: source(state.provider.mode()),
        items,
    }))
}

/// The 100 most recent audit events, newest first. Needs `read-audit` and a database.
#[utoipa::path(get, path = "/api/v1/audit", tag = "audit", responses(
    (status = 200, body = AuditList), (status = 401, body = ErrorBody),
    (status = 403, body = ErrorBody), (status = 503, body = ErrorBody)))]
async fn audit(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Json<AuditList>, ApiError> {
    require(&p, Permission::ReadAudit)?;
    let Auth::Tokens(pool) = &state.auth else {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "store_unavailable",
            "the audit log needs a database (DATABASE_URL)",
        ));
    };
    let items = apelora_persistence::recent_audit(pool, 100)
        .await?
        .into_iter()
        .map(|e| AuditItem {
            id: e.id,
            at: e
                .at
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default(),
            actor: e.actor,
            action: e.action,
            target: e.target,
            detail: e.detail,
        })
        .collect();
    Ok(Json(AuditList { items }))
}

struct BearerAuth;

impl Modify for BearerAuth {
    fn modify(&self, api: &mut utoipa::openapi::OpenApi) {
        api.components
            .get_or_insert_with(Default::default)
            .add_security_scheme(
                "bearer",
                SecurityScheme::Http(
                    HttpBuilder::new()
                        .scheme(HttpAuthScheme::Bearer)
                        .description(Some("An Apelora API token (`apl_…`)."))
                        .build(),
                ),
            );
    }
}

/// The `/api/v1` contract. `packages/api-client/openapi.json` is generated from this and the
/// TypeScript client from that file; a test fails when either is stale.
#[derive(OpenApi)]
#[openapi(
    info(title = "Apelora control plane API", description = "Inventory, audit and identity for an Apelora fabric. Inventory responses name their source (mock or Incus) and when they were observed.", license(name = "AGPL-3.0-only", identifier = "AGPL-3.0-only")),
    paths(health, whoami, cluster, instances, projects, networks, storage_pools, audit),
    modifiers(&BearerAuth),
    security(("bearer" = [])),
    tags((name = "system"), (name = "inventory"), (name = "audit"))
)]
pub struct ApiDoc;

/// The OpenAPI document as pretty JSON with a trailing newline.
pub fn openapi_json() -> String {
    let mut doc = ApiDoc::openapi();
    doc.info.version = API_VERSION.into();
    let mut s = doc
        .to_pretty_json()
        .expect("the OpenAPI document serialises");
    s.push('\n');
    s
}

async fn openapi() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "application/json")], openapi_json())
}

#[cfg(test)]
mod tests;
