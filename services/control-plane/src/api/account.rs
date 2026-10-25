// SPDX-License-Identifier: AGPL-3.0-only
//! A person's own credentials: personal tokens and active sign-ins (Console plan, Identity and
//! access, P1), and the administrators' view of everyone's.
//!
//! **Personal tokens** (`apt_…`, migration 0026) are made by a person for their own scripts. A
//! token holds no permissions of its own: each request made with it acts as the person, with the
//! role assignments and Apelora group memberships they hold at that moment, so a revoked role or a
//! disabled account takes effect on the next request. Groups that come from an identity
//! provider's token are not known without that token and do not apply. Tokens never require
//! multi-factor sign-in, so policies that demand it refuse them. Every token expires (1–365 days,
//! no default), is shown once and stored as a SHA-256 hash, and is listed with its last use.
//! Rotation makes a successor with the same name and lets the old token live for a short overlap
//! (0–24 hours, one hour by default) so a script can switch over.
//!
//! A personal token can do what its person can, except what needs the person themselves
//! ([`REFUSED_TO_PERSONAL_TOKENS`]): interactive sessions, new credentials of any kind (so a
//! leaked token cannot outlive its own expiry), approvals of someone else's request, emergency
//! access and password changes.
//!
//! **Sign-ins** are Apelora sessions (`apu_…`) issued after a password or directory sign-in.
//! People see and end their own; administrators with `user.manage` see and end anyone's. A sign-in
//! with an identity provider's token is not a session Apelora holds: it ends when the token
//! expires (at most an hour), and the provider is where it is ended earlier.
//!
//! Every change is audited in its own transaction (`personal_token.create`, `.rotate`, `.revoke`,
//! `auth.session.ended`); the token itself never is.

use apelora_persistence::personal_tokens::{self as db, PersonalTokenRecord};
use apelora_persistence::{Pool, StoreError, user_sessions};
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use super::admin::rfc3339;
use super::authz::{CallerKind, allow_platform, audit_as, begin, commit};
use super::deploy::store;
use super::sessions_auth::{ABSOLUTE_SECS, IDLE_SECS};
use super::{ApiError, AppState, ErrorBody, Principal};

/// Longest overlap a rotated token keeps, in minutes (a day).
pub const MAX_OVERLAP_MINUTES: i64 = 24 * 60;
/// The overlap when none is asked for.
pub const DEFAULT_OVERLAP_MINUTES: i64 = 60;

/// Routes a personal token may not use, whatever its person holds: they need the person
/// themselves. Checked by the route gate (`authz::route_gate`), answered with 403
/// `personal_token_refused` and audited as `access.denied`.
pub(super) const REFUSED_TO_PERSONAL_TOKENS: &[(&str, &str)] = &[
    // Interactive sessions are for signed-in people (secure-remote-access: never API tokens).
    (
        "POST",
        "/api/v1/applications/{name}/instances/{instance}/sessions",
    ),
    // New credentials: a token must not mint something that outlives it.
    ("POST", "/api/v1/me/tokens"),
    ("POST", "/api/v1/me/tokens/{id}/rotate"),
    ("POST", "/api/v1/tokens"),
    ("POST", "/api/v1/identity/service-identities"),
    ("POST", "/api/v1/identity/local-accounts"),
    ("POST", "/api/v1/identity/local-accounts/{id}/reset"),
    ("POST", "/api/v1/cluster/join-tokens"),
    ("PUT", "/api/v1/me/password"),
    // A four-eyes decision and emergency access stand for a person's judgement.
    ("POST", "/api/v1/approvals/{id}/approve"),
    ("POST", "/api/v1/identity/emergency-access"),
];

impl Principal {
    /// The credential the request was made with (see `WhoAmI::credential`).
    pub fn credential(&self) -> &'static str {
        if self.personal_token.is_some() {
            "personal-token"
        } else if self.session.is_some() {
            "session"
        } else {
            match (&self.caller, self.actor_type()) {
                (Some(c), _) if c.kind == CallerKind::User => "identity-provider",
                (Some(_), _) => "service-token",
                (None, "development") => "development",
                (None, _) => "api-token",
            }
        }
    }
}

/// The principal for a presented personal token: its person, with what they hold now.
pub(super) async fn principal_for_personal_token(
    state: &AppState,
    pool: &Pool,
    token: &str,
) -> Result<Principal, ApiError> {
    let refused = || {
        super::unauthenticated(
            "the personal token is unknown, revoked or expired, or its person is disabled",
        )
    };
    let Some(used) = db::authenticate(pool, token).await? else {
        return Err(refused());
    };
    let user = apelora_persistence::identity::user(pool, used.user_id)
        .await?
        .ok_or_else(refused)?;
    if user.issuer == apelora_persistence::local_accounts::LOCAL_ISSUER
        && !state.local_accounts_enabled()
    {
        return Err(super::unauthenticated(
            "sign-in with a local account has been turned off on this control plane, and with it this person's personal tokens",
        ));
    }
    // Groups recorded in Apelora only: an identity provider's group claims need its token.
    let groups =
        apelora_persistence::identity::groups_of(pool, user.id, None, &Default::default()).await?;
    let caller = super::caller_for(pool, CallerKind::User, user.id, groups, false).await?;
    Ok(Principal {
        name: user.username.or(user.email).unwrap_or(user.subject),
        role: None,
        caller: Some(caller),
        session: None,
        personal_token: Some(used.id),
    })
}

/// The person behind the request, or 403: tokens and sign-ins here belong to people.
fn person(p: &Principal) -> Result<Uuid, ApiError> {
    match &p.caller {
        Some(c) if c.kind == CallerKind::User => Ok(c.id),
        _ => Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "people_only",
            "personal tokens and sign-ins belong to people; sign in as a person",
        )),
    }
}

fn bad_request(msg: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, "invalid_request", msg)
}

fn no_store(mut res: Response) -> Response {
    let h = res.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    res
}

fn valid_name(name: &str) -> Result<&str, ApiError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 64 || name.chars().any(char::is_control) {
        return Err(bad_request("the name must be 1–64 characters"));
    }
    Ok(name)
}

/// The lifetime of a personal token: required, 1–365 days.
pub(super) fn personal_token_days(requested: Option<i64>) -> Result<i64, String> {
    match requested {
        None => Err("choose when the token expires: 1–365 days".into()),
        Some(d) if (1..=super::admin::MAX_TOKEN_DAYS).contains(&d) => Ok(d),
        Some(_) => Err("personal tokens expire after 1–365 days".into()),
    }
}

/// The overlap of a rotated token, in minutes: 0–1440, 60 when omitted.
pub(super) fn overlap_minutes(requested: Option<i64>) -> Result<i64, String> {
    let m = requested.unwrap_or(DEFAULT_OVERLAP_MINUTES);
    if (0..=MAX_OVERLAP_MINUTES).contains(&m) {
        Ok(m)
    } else {
        Err("the old token may keep working for 0–1440 minutes (a day)".into())
    }
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PersonalToken {
    pub id: Uuid,
    pub name: String,
    /// The person it acts as.
    pub user_id: Uuid,
    pub owner: String,
    /// `active`, `rotating` (replaced, working until `expiresAt`), `expired` or `revoked`.
    pub state: String,
    /// RFC 3339.
    pub created_at: String,
    pub expires_at: String,
    pub last_used_at: Option<String>,
    pub revoked_at: Option<String>,
    /// Who revoked it (`user:…`), or `disabled` when its person was disabled.
    pub revoked_by: Option<String>,
    /// The token this one replaced.
    pub rotated_from: Option<Uuid>,
    /// The token that replaced this one.
    pub successor: Option<Uuid>,
    /// The request was made with this token.
    pub current: bool,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PersonalTokenList {
    pub items: Vec<PersonalToken>,
    /// Live tokens a person may have at once.
    pub limit: i64,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreatePersonalToken {
    /// What it is for, e.g. "laptop backup script".
    pub name: String,
    /// 1–365 days; required.
    pub expires_in_days: Option<i64>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PersonalTokenCreated {
    /// The token itself, shown only in this response.
    pub token: String,
    pub info: PersonalToken,
}

#[derive(Deserialize, ToSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct RotatePersonalToken {
    /// How long the old token keeps working, 0–1440 minutes; 60 when omitted.
    pub overlap_minutes: Option<i64>,
    /// The successor's lifetime, 1–365 days; the old token's lifetime when omitted.
    pub expires_in_days: Option<i64>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PersonalTokenRotated {
    /// The successor's token, shown only in this response.
    pub token: String,
    pub info: PersonalToken,
    /// The old token, now expiring at the end of the overlap.
    pub previous: PersonalToken,
}

fn token_info(r: PersonalTokenRecord, p: &Principal) -> PersonalToken {
    let now = time::OffsetDateTime::now_utc();
    let state = if r.revoked_at.is_some() {
        "revoked"
    } else if r.expires_at <= now {
        "expired"
    } else if r.successor.is_some() {
        "rotating"
    } else {
        "active"
    };
    PersonalToken {
        current: p.personal_token == Some(r.id),
        id: r.id,
        name: r.name,
        user_id: r.user_id,
        owner: r.owner,
        state: state.into(),
        created_at: rfc3339(r.created_at),
        expires_at: rfc3339(r.expires_at),
        last_used_at: r.last_used_at.map(rfc3339),
        revoked_at: r.revoked_at.map(rfc3339),
        revoked_by: r.revoked_by,
        rotated_from: r.rotated_from,
        successor: r.successor,
    }
}

fn limit_reached(e: StoreError) -> ApiError {
    match e {
        StoreError::Conflict(m) => ApiError::new(StatusCode::CONFLICT, "too_many_tokens", m),
        e => e.into(),
    }
}

// ---- the person's own tokens ------------------------------------------------------------------

/// Your personal tokens (live ones, and those that ended in the last 30 days), newest first.
/// Any signed-in person.
#[utoipa::path(get, path = "/api/v1/me/tokens", tag = "identity", operation_id = "my_tokens",
    responses((status = 200, body = PersonalTokenList), (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn my_tokens(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Json<PersonalTokenList>, ApiError> {
    let me = person(&p)?;
    let items = db::list(store(&state)?, Some(me))
        .await?
        .into_iter()
        .map(|r| token_info(r, &p))
        .collect();
    Ok(Json(PersonalTokenList {
        items,
        limit: db::MAX_LIVE_PER_PERSON,
    }))
}

/// Creates a personal token, shown only in this response. It acts as you with whatever you hold
/// when it is used, and never opens terminal sessions or makes other credentials. Needs a sign-in
/// (not a personal token); 1–365 days, no default; at most 20 live tokens.
#[utoipa::path(post, path = "/api/v1/me/tokens", tag = "identity", operation_id = "create_my_token",
    request_body = CreatePersonalToken,
    responses((status = 201, body = PersonalTokenCreated), (status = 400, body = ErrorBody),
        (status = 401, body = ErrorBody), (status = 403, body = ErrorBody),
        (status = 409, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn create_my_token(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    body: Result<Json<CreatePersonalToken>, JsonRejection>,
) -> Result<Response, ApiError> {
    let me = person(&p)?;
    let Json(req) = body.map_err(|e| bad_request(e.body_text()))?;
    let name = valid_name(&req.name)?;
    let days = personal_token_days(req.expires_in_days).map_err(bad_request)?;
    let pool = store(&state)?;
    let mut tx = begin(pool).await?;
    let (secret, rec) = db::create_in(&mut tx, me, name, days)
        .await
        .map_err(limit_reached)?;
    audit_as(
        &mut tx,
        &p,
        "personal_token.create",
        &format!("personal_token:{}", rec.id),
        &json!({ "tokenName": rec.name, "expiresInDays": days }),
    )
    .await?;
    commit(tx).await?;
    Ok(no_store(
        (
            StatusCode::CREATED,
            Json(PersonalTokenCreated {
                token: secret,
                info: token_info(rec, &p),
            }),
        )
            .into_response(),
    ))
}

/// Rotates one of your live tokens: a successor with the same name (shown only in this
/// response), and the old token keeps working for the overlap (0–1440 minutes, 60 by default),
/// then expires. A token is rotated once; rotate its successor next time.
#[utoipa::path(post, path = "/api/v1/me/tokens/{id}/rotate", tag = "identity", operation_id = "rotate_my_token",
    params(("id" = Uuid, Path, description = "Token id")),
    request_body = RotatePersonalToken,
    responses((status = 201, body = PersonalTokenRotated), (status = 400, body = ErrorBody),
        (status = 401, body = ErrorBody), (status = 403, body = ErrorBody),
        (status = 404, body = ErrorBody), (status = 409, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn rotate_my_token(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(id): Path<Uuid>,
    body: Option<Json<RotatePersonalToken>>,
) -> Result<Response, ApiError> {
    let me = person(&p)?;
    let req = body.map(|Json(b)| b).unwrap_or_default();
    let overlap = overlap_minutes(req.overlap_minutes).map_err(bad_request)?;
    let pool = store(&state)?;
    let old = db::get(pool, id, Some(me)).await?;
    let days = match req.expires_in_days {
        Some(d) => personal_token_days(Some(d)).map_err(bad_request)?,
        None => ((old.expires_at - old.created_at).whole_hours() + 23)
            .div_euclid(24)
            .clamp(1, super::admin::MAX_TOKEN_DAYS),
    };
    let mut tx = begin(pool).await?;
    let r = db::rotate_in(&mut tx, id, me, days, overlap * 60)
        .await
        .map_err(|e| match e {
            StoreError::Conflict(m) if m.contains("rotated") => {
                ApiError::new(StatusCode::CONFLICT, "already_rotated", m)
            }
            e => limit_reached(e),
        })?;
    audit_as(
        &mut tx,
        &p,
        "personal_token.rotate",
        &format!("personal_token:{id}"),
        &json!({ "tokenName": r.previous.name, "successor": r.successor.id, "overlapMinutes": overlap, "expiresInDays": days }),
    )
    .await?;
    commit(tx).await?;
    Ok(no_store(
        (
            StatusCode::CREATED,
            Json(PersonalTokenRotated {
                token: r.token,
                info: token_info(r.successor, &p),
                previous: token_info(r.previous, &p),
            }),
        )
            .into_response(),
    ))
}

/// Revokes one of your live tokens; it stops working at once. Another person's token answers 404.
#[utoipa::path(delete, path = "/api/v1/me/tokens/{id}", tag = "identity", operation_id = "revoke_my_token",
    params(("id" = Uuid, Path, description = "Token id")),
    responses((status = 204), (status = 401, body = ErrorBody), (status = 403, body = ErrorBody),
        (status = 404, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn revoke_my_token(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let me = person(&p)?;
    revoke_token(&state, &p, id, Some(me)).await
}

async fn revoke_token(
    state: &AppState,
    p: &Principal,
    id: Uuid,
    owner: Option<Uuid>,
) -> Result<StatusCode, ApiError> {
    let pool = store(state)?;
    let rec = db::get(pool, id, owner).await?;
    let mut tx = begin(pool).await?;
    db::revoke(&mut *tx, id, owner, &p.actor_ref()).await?;
    audit_as(
        &mut tx,
        p,
        "personal_token.revoke",
        &format!("personal_token:{id}"),
        &json!({ "tokenName": rec.name, "owner": format!("user:{}", rec.user_id) }),
    )
    .await?;
    commit(tx).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---- administrators: everyone's tokens --------------------------------------------------------

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PersonQuery {
    /// Only this person's.
    pub user: Option<Uuid>,
}

/// Everyone's personal tokens (or one person's), live ones and those that ended in the last 30
/// days. Needs `token.manage` on the platform.
#[utoipa::path(get, path = "/api/v1/identity/personal-tokens", tag = "identity", operation_id = "personal_tokens",
    params(PersonQuery),
    responses((status = 200, body = PersonalTokenList), (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn personal_tokens(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Query(q): Query<PersonQuery>,
) -> Result<Json<PersonalTokenList>, ApiError> {
    allow_platform(&p, "token.manage")?;
    let items = db::list(store(&state)?, q.user)
        .await?
        .into_iter()
        .map(|r| token_info(r, &p))
        .collect();
    Ok(Json(PersonalTokenList {
        items,
        limit: db::MAX_LIVE_PER_PERSON,
    }))
}

/// Revokes anyone's live personal token. Needs `token.manage` on the platform.
#[utoipa::path(delete, path = "/api/v1/identity/personal-tokens/{id}", tag = "identity", operation_id = "revoke_personal_token",
    params(("id" = Uuid, Path, description = "Token id")),
    responses((status = 204), (status = 401, body = ErrorBody), (status = 403, body = ErrorBody),
        (status = 404, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn revoke_personal_token(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    allow_platform(&p, "token.manage")?;
    revoke_token(&state, &p, id, None).await
}

// ---- sign-ins -----------------------------------------------------------------------------------

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ActiveSignIn {
    /// The session id (never its token).
    pub id: Uuid,
    pub user_id: Uuid,
    pub person: String,
    /// `password` (local account) or `directory`.
    pub method: String,
    /// RFC 3339.
    pub created_at: String,
    pub last_used_at: String,
    /// The absolute limit (eight hours after sign-in).
    pub expires_at: String,
    /// When it ends unless it is used again (an hour without use, or the absolute limit).
    pub ends_at: String,
    /// The request was made with this session.
    pub current: bool,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ActiveSignInList {
    pub items: Vec<ActiveSignIn>,
    /// How this request was made (see `WhoAmI::credential`). With `identity-provider`, the
    /// current sign-in is not in the list: the provider's token is not a session Apelora holds.
    pub credential: String,
    /// A session ends after this many seconds without use.
    pub idle_secs: i64,
    /// A session ends this many seconds after sign-in, however busy.
    pub absolute_secs: i64,
}

fn sign_ins(rows: Vec<user_sessions::SessionView>, p: &Principal) -> ActiveSignInList {
    let current = p.session.as_ref().map(|s| s.id);
    ActiveSignInList {
        items: rows
            .into_iter()
            .map(|s| ActiveSignIn {
                current: Some(s.id) == current,
                id: s.id,
                user_id: s.user_id,
                person: s.person,
                method: s.method,
                created_at: rfc3339(s.created_at),
                last_used_at: rfc3339(s.last_used_at),
                expires_at: rfc3339(s.expires_at),
                ends_at: rfc3339(s.ends_at),
            })
            .collect(),
        credential: p.credential().into(),
        idle_secs: IDLE_SECS,
        absolute_secs: ABSOLUTE_SECS,
    }
}

/// Your Apelora sessions open now (password and directory sign-ins), most recently used first.
/// Sign-ins with an identity provider's token are not sessions and are not listed.
#[utoipa::path(get, path = "/api/v1/me/sign-ins", tag = "identity", operation_id = "my_sign_ins",
    responses((status = 200, body = ActiveSignInList), (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn my_sign_ins(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Json<ActiveSignInList>, ApiError> {
    let me = person(&p)?;
    let rows = user_sessions::live_sessions(store(&state)?, Some(me), IDLE_SECS).await?;
    Ok(Json(sign_ins(rows, &p)))
}

/// Ends one of your sessions (signs that browser out). Another person's answers 404.
#[utoipa::path(delete, path = "/api/v1/me/sign-ins/{id}", tag = "identity", operation_id = "end_my_sign_in",
    params(("id" = Uuid, Path, description = "Session id")),
    responses((status = 204), (status = 401, body = ErrorBody), (status = 403, body = ErrorBody),
        (status = 404, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn end_my_sign_in(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let me = person(&p)?;
    end_sign_in(&state, &p, id, Some(me)).await
}

async fn end_sign_in(
    state: &AppState,
    p: &Principal,
    id: Uuid,
    owner: Option<Uuid>,
) -> Result<StatusCode, ApiError> {
    let pool = store(state)?;
    let mut tx = begin(pool).await?;
    let Some((user, method)) = user_sessions::revoke_session_of(&mut *tx, id, owner).await? else {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "no such open session",
        ));
    };
    audit_as(
        &mut tx,
        p,
        "auth.session.ended",
        &format!("session:{id}"),
        &json!({ "person": format!("user:{user}"), "method": method }),
    )
    .await?;
    commit(tx).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Everyone's Apelora sessions open now (or one person's). Needs `user.manage` on the platform.
#[utoipa::path(get, path = "/api/v1/identity/sign-ins", tag = "identity", operation_id = "sign_ins",
    params(PersonQuery),
    responses((status = 200, body = ActiveSignInList), (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn all_sign_ins(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Query(q): Query<PersonQuery>,
) -> Result<Json<ActiveSignInList>, ApiError> {
    allow_platform(&p, "user.manage")?;
    let rows = user_sessions::live_sessions(store(&state)?, q.user, IDLE_SECS).await?;
    Ok(Json(sign_ins(rows, &p)))
}

/// Ends anyone's session. Needs `user.manage` on the platform.
#[utoipa::path(delete, path = "/api/v1/identity/sign-ins/{id}", tag = "identity", operation_id = "end_sign_in",
    params(("id" = Uuid, Path, description = "Session id")),
    responses((status = 204), (status = 401, body = ErrorBody), (status = 403, body = ErrorBody),
        (status = 404, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn end_any_sign_in(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    allow_platform(&p, "user.manage")?;
    end_sign_in(&state, &p, id, None).await
}

#[cfg(test)]
mod unit {
    #[test]
    fn personal_tokens_must_name_an_expiry_of_one_to_365_days() {
        use super::personal_token_days as days;
        assert!(days(None).is_err(), "no default");
        assert_eq!(days(Some(1)), Ok(1));
        assert_eq!(days(Some(365)), Ok(365));
        for refused in [0, -1, 366, i64::MAX] {
            assert!(days(Some(refused)).is_err(), "{refused}");
        }
    }

    #[test]
    fn a_rotated_token_overlaps_for_at_most_a_day() {
        use super::overlap_minutes as o;
        assert_eq!(o(None), Ok(60));
        assert_eq!(o(Some(0)), Ok(0));
        assert_eq!(o(Some(1440)), Ok(1440));
        assert!(o(Some(1441)).is_err());
        assert!(o(Some(-1)).is_err());
    }
}
