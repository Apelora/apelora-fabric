// SPDX-License-Identifier: AGPL-3.0-only
//! `/api/v1` routes. Every route except `/health` needs an authenticated principal and names
//! the permission it requires. Inventory responses carry their source (mock or Incus) and the
//! time they were observed, so mock data is never mistaken for a real cluster.

use std::sync::Arc;

use apelora_domain::preflight::{Check, CheckStatus};
use apelora_domain::{
    API_VERSION, ClusterMember, Instance, Network, Permission, Project, ProviderMode, Role,
    ServerInfo, StoragePool,
};
use apelora_incus_adapter::{AdapterError, Provider};
use apelora_persistence::Pool;
use authz::require;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
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
    /// Incus projects Apelora may change (default: `apelora`).
    deploy_projects: Arc<Vec<String>>,
    /// Unrestricted identity for cluster administration; `None` when not configured.
    admin: Option<Arc<Provider>>,
    /// Uplink for application OVN networks (`APELORA_NETWORK_UPLINK`); `None`: connections
    /// are not enforced.
    network_uplink: Option<Arc<str>>,
    /// Routed IPv4 range for service addresses (`APELORA_SERVICE_ADDRESSES`); `None`: no
    /// load balancers. Used only together with the uplink.
    service_range: Option<Arc<str>>,
    /// Pool behind the `data-local` volume class (`APELORA_VOLUME_POOL`); `None`: designs with
    /// volumes are refused.
    volume_pool: Option<Arc<str>>,
    /// OIDC token validation; `None`: people cannot sign in with an identity provider yet.
    oidc: Option<Arc<apelora_identity::Verifier>>,
    /// How a browser signs people in with the identity provider (public client, PKCE);
    /// `None` when OIDC sign-in is not set up.
    sign_in: Option<Arc<OidcSignIn>>,
    /// Identity providers and settings saved in the Console (`auth_config`), reloaded on change.
    auth_runtime: Arc<std::sync::RwLock<Arc<auth_config::AuthRuntime>>>,
    /// Accept legacy platform-wide `apl_…` API tokens (default). Turn off to require OIDC
    /// sign-in or automation service identities, and to put every caller under approvals.
    legacy_tokens: bool,
    /// Limits and policy for terminal and PowerShell sessions.
    console: Arc<sessions::ConsoleSettings>,
    /// Tickets and running interactive sessions (in this process's memory).
    sessions: Arc<sessions::Sessions>,
}

/// The project Apelora deploys into unless `APELORA_DEPLOY_PROJECTS` says otherwise.
pub const DEFAULT_DEPLOY_PROJECT: &str = "apelora";

impl AppState {
    pub fn new(provider: Provider, auth: Auth) -> Self {
        Self {
            provider: Arc::new(provider),
            auth,
            deploy_projects: Arc::new(vec![DEFAULT_DEPLOY_PROJECT.to_owned()]),
            admin: None,
            network_uplink: None,
            service_range: None,
            volume_pool: None,
            oidc: None,
            sign_in: None,
            auth_runtime: Arc::default(),
            legacy_tokens: true,
            console: Arc::new(sessions::ConsoleSettings::default()),
            sessions: Arc::new(sessions::Sessions::default()),
        }
    }

    pub fn with_console(mut self, settings: sessions::ConsoleSettings) -> Self {
        self.console = Arc::new(settings);
        self
    }

    pub fn with_oidc(mut self, verifier: Option<Arc<apelora_identity::Verifier>>) -> Self {
        self.oidc = verifier;
        self
    }

    pub fn with_sign_in(mut self, sign_in: Option<OidcSignIn>) -> Self {
        self.sign_in = sign_in.map(Arc::new);
        self
    }

    pub fn with_legacy_tokens(mut self, on: bool) -> Self {
        self.legacy_tokens = on;
        self
    }

    pub fn with_network_uplink(mut self, uplink: Option<String>) -> Self {
        self.network_uplink = uplink.map(Arc::from);
        self
    }

    pub fn with_service_range(mut self, range: Option<String>) -> Self {
        self.service_range = range.map(Arc::from);
        self
    }

    pub fn with_volume_pool(mut self, pool: Option<String>) -> Self {
        self.volume_pool = pool.map(Arc::from);
        self
    }

    pub fn with_admin(mut self, admin: Option<Provider>) -> Self {
        self.admin = admin.map(Arc::new);
        self
    }

    pub fn with_deploy_projects(mut self, projects: Vec<String>) -> Self {
        self.deploy_projects = Arc::new(projects);
        self
    }
}

/// The caller of a request.
#[derive(Debug, Clone)]
pub struct Principal {
    pub name: String,
    /// The role of a legacy platform-wide API token. `None` for people and service identities,
    /// whose permissions come only from their role assignments (`caller`).
    pub role: Option<Role>,
    pub caller: Option<Arc<authz::Caller>>,
}

pub fn router(state: AppState) -> Router {
    let protected = Router::new()
        .route("/api/v1/whoami", get(whoami))
        .route("/api/v1/cluster", get(cluster))
        .route(
            "/api/v1/instances",
            get(instances).post(deploy::create_instance),
        )
        .route(
            "/api/v1/instances/{project}/{name}/state",
            post(deploy::change_state),
        )
        .route(
            "/api/v1/instances/{project}/{name}",
            delete(deploy::delete_instance),
        )
        .route(
            "/api/v1/instances/{project}/{name}/move",
            get(mobility::preview).post(mobility::request),
        )
        .route(
            "/api/v1/applications",
            get(manage::list).post(manage::create),
        )
        .route(
            "/api/v1/applications/{name}",
            get(manage::detail).patch(manage::update),
        )
        .route(
            "/api/v1/applications/{name}/draft",
            get(manage::get_draft).put(manage::put_draft),
        )
        .route(
            "/api/v1/applications/{name}/versions",
            get(manage::versions).post(manage::publish),
        )
        .route(
            "/api/v1/applications/{name}/versions/{revision}",
            get(manage::version),
        )
        .route("/api/v1/applications/{name}/archive", post(manage::archive))
        .route("/api/v1/applications/{name}/restore", post(manage::restore))
        .route(
            "/api/v1/applications/{name}/operations",
            get(manage::operations).post(manage::operate),
        )
        .route(
            "/api/v1/applications/{name}/operations/{id}",
            get(manage::operation),
        )
        .route("/api/v1/applications/{name}/events", get(manage::events))
        .route("/api/v1/applications/{name}/volumes", get(volumes::list))
        .route(
            "/api/v1/applications/{name}/volumes/{volume}",
            delete(volumes::delete).patch(volumes::grow),
        )
        .route(
            "/api/v1/applications/{name}/volumes/{volume}/snapshots",
            get(volumes::snapshots).post(volumes::take_snapshot),
        )
        .route(
            "/api/v1/applications/{name}/volumes/{volume}/snapshots/{snapshot}",
            delete(volumes::delete_snapshot),
        )
        .route(
            "/api/v1/applications/{name}/volumes/{volume}/snapshots/{snapshot}/restore",
            post(volumes::restore_snapshot),
        )
        .route("/api/v1/applications/{name}/access", get(sessions::access))
        .route(
            "/api/v1/applications/{name}/instances/{instance}/sessions",
            post(sessions::open),
        )
        .route("/api/v1/sessions", get(sessions::list))
        .route("/api/v1/sessions/{id}", delete(sessions::terminate))
        .route(
            "/api/v1/applications/{name}/plan",
            post(applications::preview),
        )
        .route(
            "/api/v1/applications/{name}/deployments",
            post(applications::deploy),
        )
        .route(
            "/api/v1/applications/{name}/status",
            get(applications::status),
        )
        .route("/api/v1/jobs", get(deploy::jobs))
        .route("/api/v1/jobs/{id}", get(deploy::job))
        .route("/api/v1/tokens", get(admin::list).post(admin::create))
        .route("/api/v1/tokens/{id}", delete(admin::revoke))
        .route("/api/v1/system", get(admin::system))
        .route(
            "/api/v1/cluster/join-tokens",
            get(cluster_admin::list).post(cluster_admin::create),
        )
        .route(
            "/api/v1/cluster/join-tokens/{member}",
            delete(cluster_admin::revoke),
        )
        .route(
            "/api/v1/cluster/members/{member}/evacuation",
            get(cluster_admin::preview_evacuation),
        )
        .route(
            "/api/v1/cluster/members/{member}/evacuate",
            post(cluster_admin::evacuate),
        )
        .route(
            "/api/v1/cluster/members/{member}/restore",
            post(cluster_admin::restore),
        )
        .route(
            "/api/v1/cluster/members/{member}/removal",
            get(cluster_admin::preview_removal),
        )
        .route(
            "/api/v1/cluster/members/{member}/remove",
            post(cluster_admin::remove),
        )
        .route(
            "/api/v1/machines",
            get(machines::list).post(machines::register),
        )
        .route("/api/v1/machines/{name}", delete(machines::remove))
        .route("/api/v1/machines/{name}/approve", post(machines::approve))
        .route(
            "/api/v1/machines/{name}/approval",
            delete(machines::withdraw),
        )
        .route(
            "/api/v1/machines/{name}/admission",
            get(machines::preview_admission),
        )
        .route("/api/v1/machines/{name}/admit", post(machines::admit))
        .route("/api/v1/preflight", get(preflight))
        .route("/api/v1/projects", get(projects))
        .route("/api/v1/networks", get(networks))
        .route("/api/v1/storage-pools", get(storage_pools))
        .route("/api/v1/audit", get(audit))
        .route(
            "/api/v1/identity/organizations",
            get(identity::organizations).post(identity::create_organization),
        )
        .route("/api/v1/identity/projects", get(identity::projects))
        .route(
            "/api/v1/identity/organizations/{org}/projects",
            post(identity::create_project),
        )
        .route(
            "/api/v1/identity/projects/{project}/environments",
            get(identity::environments).post(identity::create_environment),
        )
        .route("/api/v1/identity/users", get(identity::users))
        .route("/api/v1/identity/users/lookup", get(identity::lookup_user))
        .route(
            "/api/v1/identity/users/{user}/disable",
            post(identity::disable_user),
        )
        .route(
            "/api/v1/identity/users/{user}/enable",
            post(identity::enable_user),
        )
        .route(
            "/api/v1/identity/groups",
            get(identity::groups).post(identity::create_group),
        )
        .route(
            "/api/v1/identity/groups/{group}/members",
            get(identity::group_members),
        )
        .route(
            "/api/v1/identity/groups/{group}/members/{user}",
            axum::routing::put(identity::add_member).delete(identity::remove_member),
        )
        .route(
            "/api/v1/identity/roles",
            get(identity::roles).post(identity::create_role),
        )
        .route(
            "/api/v1/identity/roles/{role}",
            delete(identity::delete_role),
        )
        .route("/api/v1/identity/permissions", get(identity::permissions))
        .route(
            "/api/v1/identity/assignments",
            get(identity::assignments).post(identity::create_assignment),
        )
        .route(
            "/api/v1/identity/assignments/preview",
            post(identity::preview_assignment),
        )
        .route(
            "/api/v1/identity/assignments/{assignment}",
            delete(identity::revoke_assignment),
        )
        .route(
            "/api/v1/identity/emergency-access",
            post(identity::emergency_access),
        )
        .route(
            "/api/v1/identity/service-identities",
            get(identity::service_identities).post(identity::create_service_identity),
        )
        .route(
            "/api/v1/identity/service-identities/{id}/disable",
            post(identity::disable_service),
        )
        .route(
            "/api/v1/identity/service-identities/{id}/enable",
            post(identity::enable_service),
        )
        .route(
            "/api/v1/identity/policies",
            get(identity::policies).put(identity::save_policy),
        )
        .route("/api/v1/identity/sign-in", get(auth_config::get_settings))
        .route("/api/v1/identity/sign-in/check", post(auth_config::check))
        .route(
            "/api/v1/identity/sign-in/providers",
            post(auth_config::create),
        )
        .route(
            "/api/v1/identity/sign-in/providers/{id}",
            put(auth_config::update).delete(auth_config::delete),
        )
        .route(
            "/api/v1/identity/sign-in/api-tokens",
            put(auth_config::set_api_tokens),
        )
        .route(
            "/api/v1/identity/policies/{id}",
            delete(identity::delete_policy),
        )
        .route("/api/v1/identity/audit", get(identity::audit_history))
        .route(
            "/api/v1/approvals",
            get(identity::approvals).post(identity::request_approval),
        )
        .route(
            "/api/v1/approvals/{id}/approve",
            post(identity::approve_request),
        )
        .route(
            "/api/v1/approvals/{id}/reject",
            post(identity::reject_request),
        )
        .route(
            "/api/v1/approvals/{id}/cancel",
            post(identity::cancel_request),
        )
        // Innermost first: the gate refuses unlisted routes, the audit layer records refusals,
        // authentication (outermost) decides who is calling.
        .route_layer(middleware::from_fn(authz::route_gate))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            authz::audit_denials,
        ))
        .route_layer(middleware::from_fn_with_state(state.clone(), authenticate));
    Router::new()
        .route("/api/v1/health", get(health))
        .route("/api/v1/sign-in", get(sign_in))
        .route("/api/v1/openapi.json", get(openapi))
        // Authenticated by its single-use ticket instead of a bearer token (browsers cannot set
        // headers on a websocket); see `sessions`.
        .route(sessions::CONNECT_PATH, get(sessions::connect))
        .merge(protected)
        .with_state(state)
        // Outermost: every request (including refusals and unknown routes) gets a request id.
        .layer(middleware::from_fn(requests::trace))
}

/// API errors with a stable machine code and a readable message.
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ApiError {
    /// The readable message (for logs).
    pub fn message(&self) -> &str {
        &self.message
    }

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
        // Logged inside the request span (request id, method, path); never the request body.
        let (code, error) = (self.code, &self.message);
        match self.status {
            StatusCode::INTERNAL_SERVER_ERROR => tracing::error!(code, %error, "request failed"),
            s if s.is_server_error() => tracing::warn!(code, %error, "request failed"),
            StatusCode::FORBIDDEN => tracing::warn!(code, %error, "request denied"),
            StatusCode::CONFLICT if code == "approval_required" => {
                tracing::info!(code, %error, "request waits for an approval")
            }
            StatusCode::CONFLICT if code.starts_with("approval_") => {
                tracing::warn!(code, %error, "approval refused")
            }
            _ => tracing::debug!(code, %error, status = self.status.as_u16(), "request refused"),
        }
        let mut res = (
            self.status,
            Json(ErrorBody {
                error: ErrorDetail {
                    code: self.code.into(),
                    message: self.message.clone(),
                },
            }),
        )
            .into_response();
        if self.status == StatusCode::UNAUTHORIZED {
            res.headers_mut()
                .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        }
        res.extensions_mut().insert(requests::ErrorCode {
            code,
            message: self.message,
        });
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
        use apelora_persistence::StoreError as E;
        match &e {
            E::NotFound(_) => ApiError::new(StatusCode::NOT_FOUND, "not_found", e.to_string()),
            E::Conflict(_) => ApiError::new(StatusCode::CONFLICT, "conflict", e.to_string()),
            // The database's own text stays in the server log; the client gets a next step.
            _ if e.is_unavailable() => {
                tracing::warn!(error = %e, "store: the database is not reachable");
                ApiError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "store_unavailable",
                    "the database is not reachable; check DATABASE_URL and that PostgreSQL is running",
                )
            }
            _ => {
                tracing::error!(error = %e, "store: database error");
                let id = requests::current();
                ApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "store_error",
                    format!(
                        "the database refused the request; the server log has the details (request id {})",
                        id.as_deref().unwrap_or("-")
                    ),
                )
            }
        }
    }
}

fn unauthenticated(message: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::UNAUTHORIZED, "unauthenticated", message)
}

async fn caller_for(
    pool: &Pool,
    kind: authz::CallerKind,
    id: uuid::Uuid,
    groups: std::collections::BTreeSet<uuid::Uuid>,
    mfa: bool,
) -> Result<Arc<authz::Caller>, ApiError> {
    use apelora_auth::{PrincipalRef, Subject};
    let mut refs = vec![match kind {
        authz::CallerKind::User => PrincipalRef::User { id },
        authz::CallerKind::Service => PrincipalRef::Service { id },
    }];
    refs.extend(groups.iter().map(|g| PrincipalRef::Group { id: *g }));
    let engine = apelora_persistence::identity::load_engine(pool, &refs).await?;
    let emergency = engine
        .assignments
        .iter()
        .any(|a| a.kind == apelora_auth::AssignmentKind::Emergency);
    let subject = match kind {
        authz::CallerKind::User => Subject {
            user: Some(id),
            groups,
            mfa,
            ..Subject::default()
        },
        authz::CallerKind::Service => Subject {
            service: Some(id),
            ..Subject::default()
        },
    };
    Ok(Arc::new(authz::Caller {
        kind,
        id,
        subject,
        engine,
        emergency,
    }))
}

async fn resolve_principal(
    state: &AppState,
    pool: &Pool,
    token: &str,
) -> Result<Principal, ApiError> {
    let fail = |e: ApiError| e;
    if apelora_identity::looks_like_jwt(token) {
        let verifier = state
            .verifier_for(apelora_identity::unverified_issuer(token).as_deref())
            .map_err(unauthenticated)?;
        let identity = verifier
            .verify(token, authz::now())
            .await
            .map_err(|e| ApiError::new(StatusCode::UNAUTHORIZED, "invalid_token", e.to_string()))?;
        let user = apelora_persistence::identity::upsert_user(
            pool,
            &identity.issuer,
            &identity.subject,
            identity.username.as_deref(),
            identity.email.as_deref(),
            identity.name.as_deref(),
        )
        .await
        .map_err(|e| fail(e.into()))?;
        if user.disabled_at.is_some() {
            let mut detail = serde_json::json!({});
            requests::tag(&mut detail);
            authz::audit_failed(
                "auth.refused",
                apelora_persistence::identity::audit_event(
                    pool,
                    &apelora_persistence::identity::AuditEntry {
                        actor: &format!("user:{}", user.id),
                        actor_type: "user",
                        action: "auth.refused",
                        target: "disabled account",
                        outcome: "denied",
                        scope: None,
                        detail: &detail,
                    },
                )
                .await,
            );
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "account_disabled",
                "this account has been disabled",
            ));
        }
        let groups = apelora_persistence::identity::groups_of(pool, user.id, &identity.groups)
            .await
            .map_err(|e| fail(e.into()))?;
        let caller = caller_for(pool, authz::CallerKind::User, user.id, groups, identity.mfa)
            .await
            .map_err(fail)?;
        let name = user.username.or(user.email).unwrap_or(identity.subject);
        return Ok(Principal {
            name,
            role: None,
            caller: Some(caller),
        });
    }
    if token.starts_with("aps_") {
        return match apelora_persistence::identity::authenticate_service(pool, token).await {
            Ok(Some(svc)) => {
                let caller = caller_for(
                    pool,
                    authz::CallerKind::Service,
                    svc.id,
                    Default::default(),
                    false,
                )
                .await
                .map_err(fail)?;
                Ok(Principal {
                    name: svc.name,
                    role: None,
                    caller: Some(caller),
                })
            }
            Ok(None) => Err(unauthenticated(
                "the service token is unknown, disabled or expired",
            )),
            Err(e) => Err(fail(e.into())),
        };
    }
    if !state.api_tokens_accepted() {
        return Err(unauthenticated(
            "legacy API tokens are disabled; sign in with your identity provider or use a service identity",
        ));
    }
    match apelora_persistence::authenticate(pool, token).await {
        Ok(Some(rec)) => match Role::parse(&rec.role) {
            Some(role) => Ok(Principal {
                name: rec.name,
                role: Some(role),
                caller: None,
            }),
            None => Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "forbidden",
                "token has an unknown role",
            )),
        },
        Ok(None) => Err(unauthenticated(
            "the API token is unknown, revoked or expired",
        )),
        Err(e) => Err(fail(e.into())),
    }
}

/// What kind of credential was presented, for logs: never any part of the token.
fn credential_kind(token: &str) -> &'static str {
    if apelora_identity::looks_like_jwt(token) {
        "identity-provider token"
    } else if token.starts_with("aps_") {
        "service token"
    } else if token.starts_with("apl_") {
        "API token"
    } else {
        "unrecognised"
    }
}

async fn authenticate(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    let principal = match &state.auth {
        Auth::DisabledForDevelopment => Principal {
            name: "development".into(),
            role: Some(Role::FabricAdmin),
            caller: None,
        },
        Auth::Tokens(pool) => {
            let presented = req
                .headers()
                .get(header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Bearer "))
                .map(str::trim)
                .filter(|t| !t.is_empty());
            let Some(token) = presented else {
                // Method, path and request id come from the request span.
                tracing::debug!("request without a bearer token");
                return unauthenticated(
                    "a bearer token is required (Authorization: Bearer <token>)",
                )
                .into_response();
            };
            match resolve_principal(&state, pool, token).await {
                Ok(p) => p,
                Err(e) => {
                    // Refused credentials are logged (never the token itself), so a sign-in
                    // problem can be traced from the server side. They are not audited: an
                    // audit row per bad request would let anyone fill the log.
                    if !e.status.is_server_error() {
                        let issuer = apelora_identity::looks_like_jwt(token)
                            .then(|| apelora_identity::unverified_issuer(token))
                            .flatten();
                        tracing::warn!(
                            credential = credential_kind(token),
                            issuer = issuer.as_deref().unwrap_or("-"),
                            code = e.code,
                            reason = %e.message,
                            "credential refused"
                        );
                    }
                    return e.into_response();
                }
            }
        }
    };
    if principal.caller.as_ref().is_some_and(|c| c.emergency)
        && let Auth::Tokens(pool) = &state.auth
    {
        // Everything done while holding emergency access is on the record.
        authz::audit_failed(
            "emergency_access.request",
            authz::audit_as(
                pool,
                &principal,
                "emergency_access.request",
                &format!("{} {}", req.method(), req.uri().path()),
                &serde_json::json!({}),
            )
            .await,
        );
    }
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
#[serde(rename_all = "camelCase")]
pub struct WhoAmI {
    pub name: String,
    /// `user` (OIDC sign-in), `service` (automation identity), `legacy-token` or `development`.
    pub kind: String,
    /// The user or service identity id; absent for legacy tokens.
    pub id: Option<uuid::Uuid>,
    /// The role of a legacy API token; `null` for people and service identities.
    pub role: Option<Role>,
    /// What the older, coarse permissions amount to for this caller at platform level.
    pub permissions: Vec<Permission>,
    /// Permission ids held at some scope (what the UI may offer; the server decides each request).
    pub capabilities: Vec<String>,
    /// Permission ids held across the whole platform.
    pub platform_permissions: Vec<String>,
    /// The role assignments in force for this caller.
    pub roles: Vec<RoleGrant>,
    /// Incus projects this control plane deploys into (`APELORA_DEPLOY_PROJECTS`).
    pub deploy_projects: Vec<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RoleGrant {
    pub role: String,
    #[schema(value_type = Object)]
    pub scope: apelora_auth::Scope,
    pub via_group: bool,
    pub emergency: bool,
    /// RFC 3339.
    pub expires_at: Option<String>,
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
pub struct PreflightReport {
    pub source: Source,
    /// The worst status among the checks.
    pub overall: CheckStatus,
    pub checks: Vec<Check>,
}

/// Read-only readiness checks of the Incus server or cluster and of the projects Apelora
/// deploys into. Changes nothing. Needs `read-inventory`.
#[utoipa::path(get, path = "/api/v1/preflight", tag = "inventory", responses(
    (status = 200, body = PreflightReport), (status = 401, body = ErrorBody),
    (status = 403, body = ErrorBody), (status = 502, body = ErrorBody)))]
async fn preflight(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Json<PreflightReport>, ApiError> {
    require(&p, Permission::ReadInventory)?;
    let pr = &state.provider;
    let server = pr.server_info().await?;
    let (members, projects, pools, networks) = tokio::try_join!(
        pr.cluster_members(),
        pr.projects(),
        pr.storage_pools(),
        pr.networks()
    )?;
    let checks = apelora_domain::preflight::evaluate(&apelora_domain::preflight::Observed {
        server: &server,
        members: &members,
        projects: &projects,
        pools: &pools,
        networks: &networks,
        deploy_projects: &state.deploy_projects,
        volume_pool: state.volume_pool.as_deref(),
    });
    Ok(Json(PreflightReport {
        source: source(pr.mode()),
        overall: apelora_domain::preflight::overall(&checks),
        checks,
    }))
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuditItem {
    pub id: i64,
    /// RFC 3339 timestamp.
    pub at: String,
    /// Stable id of who acted, e.g. `user:<uuid>`, `service:<uuid>`, `token:<name>`.
    pub actor: String,
    /// `user`, `service`, `token`, `development`, `cli` or `system`.
    pub actor_type: String,
    pub action: String,
    pub target: String,
    /// `success`, `denied` or `failed`.
    pub outcome: String,
    /// The organisation, project and application the event concerns, when it concerns any.
    #[schema(value_type = Option<Object>)]
    pub scope: Option<Value>,
    #[schema(value_type = Object)]
    pub detail: Value,
}

impl AuditItem {
    pub(crate) fn from_row(e: apelora_persistence::identity::AuditRow) -> Self {
        Self {
            id: e.id,
            at: e
                .at
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default(),
            actor: e.actor,
            actor_type: e.actor_type,
            action: e.action,
            target: e.target,
            outcome: e.outcome,
            scope: e.scope,
            detail: e.detail,
        }
    }
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

/// What a browser needs to sign a person in with the identity provider: an OpenID Connect
/// authorization-code flow with PKCE for a public client. Nothing here is secret.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OidcSignIn {
    /// The provider's issuer; its discovery document is at `<issuer>/.well-known/openid-configuration`.
    pub issuer: String,
    /// The public client registered for the Fabric Console and the Studio (no client secret).
    pub client_id: String,
    pub scopes: Vec<String>,
    /// What the button says after "Sign in with", e.g. the organisation's name.
    pub label: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SignInOptions {
    /// The organisation's identity providers people can sign in with, in order.
    pub providers: Vec<OidcSignIn>,
    /// Whether API tokens are accepted (legacy platform-wide tokens).
    pub api_tokens: bool,
}

/// Public. How people can sign in to this control plane.
#[utoipa::path(get, path = "/api/v1/sign-in", tag = "system", security(()), operation_id = "sign_in_options",
    responses((status = 200, body = SignInOptions)))]
async fn sign_in(State(state): State<AppState>) -> Json<SignInOptions> {
    Json(SignInOptions {
        providers: state.sign_in_providers(),
        api_tokens: state.api_tokens_accepted(),
    })
}

/// Who is calling, with the role assignments and permissions in force.
#[utoipa::path(get, path = "/api/v1/whoami", tag = "system",
    responses((status = 200, body = WhoAmI), (status = 401, body = ErrorBody)))]
async fn whoami(State(state): State<AppState>, Extension(p): Extension<Principal>) -> Json<WhoAmI> {
    let platform = apelora_auth::Resource::platform();
    let permissions = [
        Permission::ReadInventory,
        Permission::ReadAudit,
        Permission::ManageTokens,
        Permission::ManageInfrastructure,
        Permission::DesignApplications,
        Permission::Deploy,
    ]
    .into_iter()
    .filter(|x| require(&p, *x).is_ok())
    .collect();
    let ids = || apelora_auth::PERMISSIONS.iter().map(|d| d.id);
    let platform_permissions = ids()
        .filter(|id| authz::allow(&p, id, &platform).is_ok())
        .map(str::to_owned)
        .collect();
    let capabilities = match &p.caller {
        Some(c) => ids()
            .filter(|id| c.engine.holds_anywhere(&c.subject, id, authz::now()))
            .map(str::to_owned)
            .collect(),
        None => ids()
            .filter(|id| authz::allow(&p, id, &platform).is_ok())
            .map(str::to_owned)
            .collect(),
    };
    let now = authz::now();
    let roles = p
        .caller
        .as_ref()
        .map(|c| {
            c.engine
                .assignments
                .iter()
                .filter(|a| !a.revoked && a.expires_at.is_none_or(|t| t > now))
                .map(|a| RoleGrant {
                    role: c
                        .engine
                        .resolve(&a.role)
                        .map(|r| r.name)
                        .unwrap_or_default(),
                    scope: a.scope.clone(),
                    via_group: matches!(a.principal, apelora_auth::PrincipalRef::Group { .. }),
                    emergency: a.kind == apelora_auth::AssignmentKind::Emergency,
                    expires_at: a
                        .expires_at
                        .and_then(|t| time::OffsetDateTime::from_unix_timestamp(t).ok())
                        .and_then(|t| {
                            t.format(&time::format_description::well_known::Rfc3339)
                                .ok()
                        }),
                })
                .collect()
        })
        .unwrap_or_default();
    Json(WhoAmI {
        kind: match (&p.caller, p.actor_type()) {
            (Some(_), t) => t.to_owned(),
            (None, "development") => "development".to_owned(),
            (None, _) => "legacy-token".to_owned(),
        },
        id: p.caller.as_ref().map(|c| c.id),
        name: p.name,
        role: p.role,
        permissions,
        capabilities,
        platform_permissions,
        roles,
        deploy_projects: state.deploy_projects.to_vec(),
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

/// The most recent audit events the caller may read, newest first. Needs `audit.read`
/// (platform-wide, or limited to the events of the caller's organisations, projects and
/// applications) and a database.
#[utoipa::path(get, path = "/api/v1/audit", tag = "audit", responses(
    (status = 200, body = AuditList), (status = 401, body = ErrorBody),
    (status = 403, body = ErrorBody), (status = 503, body = ErrorBody)))]
async fn audit(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Json<AuditList>, ApiError> {
    let filter = authz::filter_for(&p, "audit.read");
    if filter.is_empty() {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            "you may not read the audit log (audit.read)",
        ));
    }
    let Auth::Tokens(pool) = &state.auth else {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "store_unavailable",
            "the audit log needs a database (DATABASE_URL)",
        ));
    };
    let items = apelora_persistence::identity::audit_history(pool, &filter, None, None, None, 100)
        .await?
        .into_iter()
        .map(AuditItem::from_row)
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
                        .description(Some("An OIDC access token from the identity provider, an automation service token (`aps_…`) or a legacy API token (`apl_…`)."))
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
    paths(health, sign_in, whoami, cluster, instances, projects, networks, storage_pools, preflight, audit,
        deploy::create_instance, deploy::change_state, deploy::delete_instance, mobility::preview, mobility::request, deploy::job, deploy::jobs,
        admin::list, admin::create, admin::revoke, admin::system,
        manage::list, manage::create, manage::detail, manage::update, manage::get_draft, manage::put_draft, manage::versions, manage::publish, manage::version, manage::archive, manage::restore, manage::operations, manage::operate, manage::operation, manage::events, volumes::list, volumes::grow, volumes::delete, volumes::snapshots, volumes::take_snapshot, volumes::restore_snapshot, volumes::delete_snapshot, applications::preview, applications::deploy, applications::status,
        cluster_admin::create, cluster_admin::list, cluster_admin::revoke,
        cluster_admin::preview_evacuation, cluster_admin::evacuate, cluster_admin::restore,
        cluster_admin::preview_removal, cluster_admin::remove,
        machines::list, machines::register, machines::approve, machines::withdraw,
        machines::preview_admission, machines::admit, machines::remove,
        identity::organizations, identity::create_organization, identity::projects, identity::create_project,
        identity::environments, identity::create_environment, identity::users, identity::lookup_user,
        identity::disable_user, identity::enable_user, identity::groups, identity::create_group,
        identity::group_members, identity::add_member, identity::remove_member, identity::roles,
        identity::create_role, identity::delete_role, identity::permissions, identity::assignments,
        identity::create_assignment, identity::preview_assignment, identity::revoke_assignment,
        identity::emergency_access, identity::service_identities, identity::create_service_identity,
        identity::disable_service, identity::enable_service, identity::policies, identity::save_policy,
        identity::delete_policy, auth_config::get_settings, auth_config::check, auth_config::create, auth_config::update, auth_config::delete, auth_config::set_api_tokens, identity::audit_history, identity::approvals, identity::request_approval,
        identity::approve_request, identity::reject_request, identity::cancel_request,
        sessions::access, sessions::open, sessions::list, sessions::terminate),
    modifiers(&BearerAuth),
    security(("bearer" = [])),
    tags((name = "system"), (name = "inventory"), (name = "deployment"), (name = "applications"), (name = "cluster"), (name = "administration"), (name = "audit"), (name = "identity"), (name = "sessions"))
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

mod admin;
mod applications;
pub mod auth_config;
mod authz;
mod cluster_admin;
mod deploy;
mod document;
mod identity;
mod machines;
mod manage;
mod mobility;
pub mod requests;
pub mod sessions;
mod volumes;

#[cfg(test)]
mod identity_tests;
#[cfg(test)]
mod manage_tests;
#[cfg(test)]
mod sessions_tests;
#[cfg(test)]
mod tests;
