// SPDX-License-Identifier: AGPL-3.0-only
//! Cluster membership: join tokens for new Incus members (ADR 0003).
//!
//! A fabric administrator approves a new member by asking for a join token for its name. The
//! token is single-use, expires, and carries a secret: it is returned once, never stored,
//! logged or audited, and the new server presents it to `incus admin init` itself. Unlike
//! deployments, these calls reach Incus directly (through the separate, unrestricted admin
//! identity): a queued job would have to store the secret.

use apelora_domain::{PendingJoin, Permission, is_valid_name};
use apelora_incus_adapter::Provider;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;

use super::{ApiError, AppState, Auth, ErrorBody, Principal, Source, require, source};

/// Join tokens that stay valid longer than this get a warning.
const RECOMMENDED_MAX_LIFETIME: time::Duration = time::Duration::hours(1);

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateJoinToken {
    /// Name the new member will join with.
    pub member_name: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct JoinTokenCreated {
    pub member_name: String,
    /// RFC 3339, as Incus reports it.
    pub expires_at: String,
    /// Single-use secret for `incus admin init` on the new server. Shown only in this response.
    pub token: String,
    /// E.g. that the token lives longer than recommended.
    pub warnings: Vec<String>,
}

#[derive(Serialize, ToSchema)]
pub struct JoinTokenList {
    pub source: Source,
    pub items: Vec<PendingJoin>,
}

fn admin(state: &AppState) -> Result<&Provider, ApiError> {
    state.admin.as_deref().ok_or_else(|| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "cluster_admin_not_configured",
            "cluster administration needs an unrestricted Incus identity \
             (APELORA_INCUS_ADMIN_CLIENT_CERT and APELORA_INCUS_ADMIN_CLIENT_KEY)",
        )
    })
}

/// Membership changes are always audited, so they need the database.
fn audit_store(state: &AppState) -> Result<&apelora_persistence::Pool, ApiError> {
    match &state.auth {
        Auth::Tokens(pool) => Ok(pool),
        Auth::DisabledForDevelopment => Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "store_unavailable",
            "cluster administration is audited and needs a database (DATABASE_URL)",
        )),
    }
}

fn member_name(name: &str) -> Result<(), ApiError> {
    if is_valid_name(name) {
        Ok(())
    } else {
        Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            format!("invalid member name {name:?}"),
        ))
    }
}

/// Warnings about a token's lifetime; none when the expiry cannot be read (e.g. mock data).
pub(super) fn lifetime_warnings(expires_at: &str, now: time::OffsetDateTime) -> Vec<String> {
    let rfc3339 = &time::format_description::well_known::Rfc3339;
    match time::OffsetDateTime::parse(expires_at, rfc3339) {
        Ok(t) if t - now > RECOMMENDED_MAX_LIFETIME => vec![format!(
            "this token stays valid until {expires_at}; set the Incus option \
             cluster.join_token_expiry to 1H or less, and revoke tokens that are not used"
        )],
        _ => Vec::new(),
    }
}

/// Approves a new cluster member: asks Incus for a single-use join token. Needs
/// `manage-infrastructure`. The token is in this response only.
#[utoipa::path(post, path = "/api/v1/cluster/join-tokens", tag = "cluster", operation_id = "create_join_token", request_body = CreateJoinToken,
    responses((status = 201, body = JoinTokenCreated), (status = 400, body = ErrorBody),
        (status = 401, body = ErrorBody), (status = 403, body = ErrorBody),
        (status = 502, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn create(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    body: Result<Json<CreateJoinToken>, JsonRejection>,
) -> Result<Response, ApiError> {
    require(&p, Permission::ManageInfrastructure)?;
    let Json(req) =
        body.map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, "invalid_request", e.body_text()))?;
    member_name(&req.member_name)?;
    let pool = audit_store(&state)?;
    let provider = admin(&state)?;
    let t = provider.create_join_token(&req.member_name).await?;
    apelora_persistence::audit(
        pool,
        &p.name,
        "cluster.join-token.created",
        &t.member_name,
        &json!({ "role": p.role, "expiresAt": t.expires_at }),
    )
    .await?;
    let warnings = lifetime_warnings(&t.expires_at, time::OffsetDateTime::now_utc());
    let mut res = (
        StatusCode::CREATED,
        Json(JoinTokenCreated {
            member_name: t.member_name,
            expires_at: t.expires_at,
            token: t.token,
            warnings,
        }),
    )
        .into_response();
    let h = res.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    Ok(res)
}

/// Join tokens waiting to be used (no secrets). Needs `manage-infrastructure`.
#[utoipa::path(get, path = "/api/v1/cluster/join-tokens", tag = "cluster", operation_id = "list_join_tokens",
    responses((status = 200, body = JoinTokenList), (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody), (status = 502, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn list(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Json<JoinTokenList>, ApiError> {
    require(&p, Permission::ManageInfrastructure)?;
    let provider = admin(&state)?;
    let items = provider.join_tokens().await?;
    Ok(Json(JoinTokenList {
        source: source(provider.mode()),
        items,
    }))
}

/// Withdraws a new member's approval: revokes its unused join token. Needs
/// `manage-infrastructure`.
#[utoipa::path(delete, path = "/api/v1/cluster/join-tokens/{member}", tag = "cluster", operation_id = "revoke_join_token",
    params(("member" = String, Path, description = "Name the token was issued for")),
    responses((status = 204), (status = 400, body = ErrorBody), (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody), (status = 404, body = ErrorBody),
        (status = 502, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn revoke(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(member): Path<String>,
) -> Result<StatusCode, ApiError> {
    require(&p, Permission::ManageInfrastructure)?;
    member_name(&member)?;
    let pool = audit_store(&state)?;
    let provider = admin(&state)?;
    if !provider.revoke_join_token(&member).await? {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            format!("no pending join token for {member}"),
        ));
    }
    apelora_persistence::audit(
        pool,
        &p.name,
        "cluster.join-token.revoked",
        &member,
        &json!({ "role": p.role }),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
