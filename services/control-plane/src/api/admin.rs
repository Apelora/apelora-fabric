// SPDX-License-Identifier: AGPL-3.0-only
//! Administration: API tokens (create, list, revoke) and what this control plane runs.
//!
//! A new token is returned once (`Cache-Control: no-store`); only its hash is stored and it
//! is never logged or audited. Creating and revoking tokens is audited with the same actions
//! as the command line (`token.create`, `token.revoke`).

use apelora_domain::{Permission, Role};
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;
use uuid::Uuid;

use super::{API_VERSION, ApiError, AppState, Auth, ErrorBody, Principal, require};

/// Tokens created through the API always expire; at most a year, 90 days by default.
const DEFAULT_TOKEN_DAYS: i64 = 90;
const MAX_TOKEN_DAYS: i64 = 365;

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TokenInfo {
    pub id: Uuid,
    pub name: String,
    pub role: Role,
    /// RFC 3339.
    pub created_at: String,
    pub expires_at: Option<String>,
    pub revoked_at: Option<String>,
    pub last_used_at: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct TokenList {
    pub items: Vec<TokenInfo>,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateToken {
    /// Who or what uses it, e.g. "ci" or a person's name.
    pub name: String,
    pub role: Role,
    /// 1–365 days; 90 when omitted.
    pub expires_in_days: Option<i64>,
}

#[derive(Serialize, ToSchema)]
pub struct TokenCreated {
    /// The token itself, shown only in this response.
    pub token: String,
    pub info: TokenInfo,
}

/// Facts about this control plane and the Incus release it is pinned to.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SystemInfo {
    pub version: String,
    pub api_version: String,
    /// Incus release Apelora is tested against (`upstream.lock.yaml`).
    pub incus_pinned: String,
    /// Incus LTS series the pin follows, e.g. "7.0".
    pub incus_series: String,
    /// Evidence recorded for the pin.
    pub incus_tested_with: String,
    /// The certificates Apelora uses to reach Incus and when they expire (empty with the mock).
    pub certificates: Vec<CertificateStatus>,
}

/// Certificates expiring sooner than this get a warning.
const CERTIFICATE_WARNING_DAYS: i64 = 30;

#[derive(Serialize, ToSchema, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CertificateStatus {
    /// `deploy` (restricted client identity), `admin` (unrestricted client identity) or
    /// `server` (the pinned Incus server certificate).
    pub certificate: String,
    /// RFC 3339.
    pub expires_at: String,
    /// Whole days left; negative once expired.
    pub days_left: i64,
    /// `ok`, `expiring` (within 30 days) or `expired`.
    pub state: String,
}

pub(super) fn certificate_status(certificate: &str, not_after: i64, now: i64) -> CertificateStatus {
    let days_left = (not_after - now).div_euclid(86_400);
    CertificateStatus {
        certificate: certificate.into(),
        expires_at: time::OffsetDateTime::from_unix_timestamp(not_after)
            .map(rfc3339)
            .unwrap_or_default(),
        days_left,
        state: if not_after <= now {
            "expired"
        } else if days_left < CERTIFICATE_WARNING_DAYS {
            "expiring"
        } else {
            "ok"
        }
        .into(),
    }
}

pub(super) fn rfc3339(t: time::OffsetDateTime) -> String {
    t.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

fn info(r: apelora_persistence::TokenRecord) -> TokenInfo {
    TokenInfo {
        id: r.id,
        role: Role::parse(&r.role).unwrap_or(Role::Auditor),
        name: r.name,
        created_at: rfc3339(r.created_at),
        expires_at: r.expires_at.map(rfc3339),
        revoked_at: r.revoked_at.map(rfc3339),
        last_used_at: r.last_used_at.map(rfc3339),
    }
}

fn store(state: &AppState) -> Result<&apelora_persistence::Pool, ApiError> {
    match &state.auth {
        Auth::Tokens(pool) => Ok(pool),
        Auth::DisabledForDevelopment => Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "store_unavailable",
            "API tokens need a database (DATABASE_URL)",
        )),
    }
}

fn bad_request(msg: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, "invalid_request", msg)
}

/// API tokens, active and revoked (never the tokens themselves). Needs `manage-tokens`.
#[utoipa::path(get, path = "/api/v1/tokens", tag = "administration", operation_id = "tokens",
    responses((status = 200, body = TokenList), (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn list(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Json<TokenList>, ApiError> {
    require(&p, Permission::ManageTokens)?;
    let items = apelora_persistence::list_tokens(store(&state)?)
        .await?
        .into_iter()
        .map(info)
        .collect();
    Ok(Json(TokenList { items }))
}

/// Creates an API token, shown only in this response. Needs `manage-tokens`.
#[utoipa::path(post, path = "/api/v1/tokens", tag = "administration", operation_id = "create_token",
    request_body = CreateToken,
    responses((status = 201, body = TokenCreated), (status = 400, body = ErrorBody),
        (status = 401, body = ErrorBody), (status = 403, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn create(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    body: Result<Json<CreateToken>, JsonRejection>,
) -> Result<Response, ApiError> {
    require(&p, Permission::ManageTokens)?;
    let Json(req) = body.map_err(|e| bad_request(e.body_text()))?;
    let name = req.name.trim();
    if name.is_empty() || name.chars().count() > 64 || name.chars().any(char::is_control) {
        return Err(bad_request("the name must be 1–64 characters"));
    }
    let days = req.expires_in_days.unwrap_or(DEFAULT_TOKEN_DAYS);
    if !(1..=MAX_TOKEN_DAYS).contains(&days) {
        return Err(bad_request(format!(
            "tokens expire after 1–{MAX_TOKEN_DAYS} days"
        )));
    }
    let pool = store(&state)?;
    let expires = time::OffsetDateTime::now_utc() + time::Duration::days(days);
    let (secret, rec) =
        apelora_persistence::create_token(pool, name, req.role.as_str(), Some(expires)).await?;
    apelora_persistence::audit(
        pool,
        &p.name,
        "token.create",
        &rec.name,
        &json!({ "id": rec.id, "role": rec.role, "by": p.role }),
    )
    .await?;
    let mut res = (
        StatusCode::CREATED,
        Json(TokenCreated {
            token: secret,
            info: info(rec),
        }),
    )
        .into_response();
    let h = res.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    Ok(res)
}

/// Revokes an API token; it stops working at once. Needs `manage-tokens`.
#[utoipa::path(delete, path = "/api/v1/tokens/{id}", tag = "administration", operation_id = "revoke_token",
    params(("id" = Uuid, Path, description = "Token id")),
    responses((status = 204), (status = 401, body = ErrorBody), (status = 403, body = ErrorBody),
        (status = 404, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn revoke(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    require(&p, Permission::ManageTokens)?;
    let pool = store(&state)?;
    apelora_persistence::revoke_token(pool, id).await?;
    apelora_persistence::audit(
        pool,
        &p.name,
        "token.revoke",
        &id.to_string(),
        &json!({ "by": p.role }),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The value of `key:` inside the `incus:` block of `upstream.lock.yaml`.
fn lock_value(lock: &str, key: &str) -> String {
    let mut inside = false;
    for line in lock.lines() {
        if line == "  incus:" {
            inside = true;
            continue;
        }
        if inside && line.starts_with("  ") && !line.starts_with("    ") {
            break;
        }
        if inside && let Some(v) = line.trim().strip_prefix(&format!("{key}:")) {
            return v.trim().trim_matches('"').to_owned();
        }
    }
    String::new()
}

/// Versions of this control plane and the Incus release it is pinned to, and when the
/// certificates Apelora uses to reach Incus expire. Needs `read-inventory`. The running Incus
/// version is in `/api/v1/health`.
#[utoipa::path(get, path = "/api/v1/system", tag = "system", operation_id = "system",
    responses((status = 200, body = SystemInfo), (status = 401, body = ErrorBody), (status = 403, body = ErrorBody)))]
pub(super) async fn system(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Json<SystemInfo>, ApiError> {
    require(&p, Permission::ReadInventory)?;
    const LOCK: &str = include_str!("../../../../upstream.lock.yaml");
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    // The two client identities first, then the pinned server certificate.
    let deploy = state.provider.certificate_expiry();
    let mut certificates: Vec<CertificateStatus> = deploy
        .map(|e| certificate_status("deploy", e.client, now))
        .into_iter()
        .collect();
    if let Some(e) = state.admin.as_deref().and_then(|a| a.certificate_expiry()) {
        certificates.push(certificate_status("admin", e.client, now));
    }
    certificates.extend(deploy.map(|e| certificate_status("server", e.server, now)));
    Ok(Json(SystemInfo {
        version: env!("CARGO_PKG_VERSION").into(),
        api_version: API_VERSION.into(),
        incus_pinned: lock_value(LOCK, "pinned_tag"),
        incus_series: lock_value(LOCK, "series"),
        incus_tested_with: lock_value(LOCK, "tested_with"),
        certificates,
    }))
}

#[cfg(test)]
mod tests {
    #[test]
    fn certificates_warn_a_month_ahead_and_say_when_they_expired() {
        let now = 1_800_000_000;
        let day = 86_400;
        let s = |t| super::certificate_status("deploy", t, now);
        assert_eq!(
            (
                s(now + 90 * day).days_left,
                s(now + 90 * day).state.as_str()
            ),
            (90, "ok")
        );
        assert_eq!(s(now + 29 * day + 5).state, "expiring");
        assert_eq!(
            (s(now - day).days_left, s(now - day).state.as_str()),
            (-1, "expired")
        );
        assert!(s(now).expires_at.starts_with("2027-01-15"));
    }

    #[test]
    fn the_pinned_incus_release_is_read_from_the_lock_file() {
        const LOCK: &str = include_str!("../../../../upstream.lock.yaml");
        assert!(super::lock_value(LOCK, "pinned_tag").starts_with("v7.0."));
        assert_eq!(super::lock_value(LOCK, "series"), "7.0");
        assert_eq!(super::lock_value(LOCK, "missing"), "");
    }
}
