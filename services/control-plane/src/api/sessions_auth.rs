// SPDX-License-Identifier: AGPL-3.0-only
//! Local accounts and Apelora sessions (ADR 0007, docs/design/sign-in-methods.md).
//!
//! A person with a local account signs in with a username and password and gets an Apelora
//! session token (`apu_…`), which [`issue_session`] alone creates (the directory sign-in uses it
//! too). Passwords are hashed with Argon2id (19 MiB, 2 passes, 1 lane) off the request thread;
//! a sign-in for an unknown, disabled or locked account verifies against a dummy hash so it
//! takes about as long and answers the same. Five failures in a row lock an account for 15
//! minutes, and every client address may try at most 10 times a minute.
//!
//! Administrators never learn a password: creating or resetting an account returns a one-time
//! setup link (24 hours), and the person chooses the password. Passwords, setup tokens and
//! session tokens are never logged, audited or echoed in an error.

use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, Ipv4Addr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use apelora_persistence::identity as idb;
use apelora_persistence::local_accounts::{self as db, LocalLogin};
use apelora_persistence::user_sessions;
use apelora_persistence::{Pool, StoreError};
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use argon2::{Algorithm, Argon2, Params, Version};
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use super::authz::{
    CallerKind, allow_platform, audit_alone, audit_as, audit_failed, begin, commit, guard,
};
use super::deploy::store;
use super::identity::{UserDto, user_dto};
use super::sessions::ClientAddress;
use super::{ApiError, AppState, ErrorBody, Principal};

/// A session ends after this long without a request.
pub const IDLE_SECS: i64 = 60 * 60;
/// A session ends this long after sign-in, however busy.
pub const ABSOLUTE_SECS: i64 = 8 * 60 * 60;
/// Password length, in characters.
pub const MIN_PASSWORD: usize = 12;
pub const MAX_PASSWORD: usize = 128;
/// Sign-in attempts (password, setup, password change) per client address per minute.
pub const ATTEMPTS_PER_MINUTE: usize = 10;
/// The saved key for "local accounts are accepted" (default off).
pub const LOCAL_ACCOUNTS: &str = "auth.local_accounts";

/// The rule people see when a password is refused.
pub const PASSWORD_RULE: &str =
    "Use 12 to 128 characters, not your username. A few unrelated words work well.";
const INVALID_CREDENTIALS: &str = "The username or password is incorrect.";

// ---- sessions -----------------------------------------------------------------------------------

/// How a person signed in to get an Apelora session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionMethod {
    Password,
    Directory,
}

impl SessionMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            SessionMethod::Password => "password",
            SessionMethod::Directory => "directory",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "password" => Some(SessionMethod::Password),
            "directory" => Some(SessionMethod::Directory),
            _ => None,
        }
    }
}

/// The session a request was made with (never the token).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionRef {
    pub id: Uuid,
    pub method: SessionMethod,
}

/// A new session: the token is handed to the person once and stored only as a hash.
#[derive(Debug, Clone)]
pub struct IssuedSession {
    pub token: String,
    pub expires_at: OffsetDateTime,
}

/// The one place Apelora sessions are created, for password and directory sign-ins.
/// `external_groups` are directory group names, mapped onto Apelora groups by external name
/// whenever the session is used.
pub async fn issue_session(
    tx: &mut apelora_persistence::PgConnection,
    user_id: Uuid,
    method: SessionMethod,
    external_groups: &[String],
) -> Result<IssuedSession, ApiError> {
    let (token, rec) = user_sessions::create_session_in(
        tx,
        user_id,
        method.as_str(),
        external_groups,
        ABSOLUTE_SECS,
    )
    .await?;
    tracing::info!(user = %user_id, session = %rec.id, method = method.as_str(), "session issued");
    Ok(IssuedSession {
        token,
        expires_at: rec.expires_at,
    })
}

/// The caller behind an `apu_` token, if the session is live.
pub(super) async fn principal_for_session(
    state: &AppState,
    pool: &Pool,
    token: &str,
) -> Result<Principal, ApiError> {
    let ended = || {
        super::unauthenticated(
            "the session has ended (signed out, an hour without use, or eight hours after sign-in); sign in again",
        )
    };
    let Some(s) = user_sessions::authenticate_session(pool, token, IDLE_SECS).await? else {
        return Err(ended());
    };
    let method = SessionMethod::parse(&s.method).ok_or_else(ended)?;
    if method == SessionMethod::Password && !state.local_accounts_enabled() {
        return Err(super::unauthenticated(
            "sign-in with a local account has been turned off on this control plane",
        ));
    }
    let user = idb::user(pool, s.user_id).await?.ok_or_else(ended)?;
    let groups = idb::groups_of(pool, user.id, &s.external_groups.into_iter().collect()).await?;
    let caller = super::caller_for(pool, CallerKind::User, user.id, groups, false).await?;
    Ok(Principal {
        name: user.username.or(user.email).unwrap_or(user.subject),
        role: None,
        caller: Some(caller),
        session: Some(SessionRef { id: s.id, method }),
        personal_token: None,
    })
}

// ---- passwords ----------------------------------------------------------------------------------

/// Argon2id, memory 19 MiB (19 456 KiB), 2 passes, 1 lane.
fn argon2() -> Argon2<'static> {
    let params = Params::new(19_456, 2, 1, None).unwrap_or_default();
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

/// Passwords longer than this many bytes are not hashed at all (they cannot match: the policy
/// allows at most 128 characters); a fixed string is hashed instead so the time is the same.
const MAX_PASSWORD_BYTES: usize = MAX_PASSWORD * 4;

/// Times the dummy hash was verified (unknown, disabled, locked or not-yet-set-up accounts).
/// Tests read it to check that those paths cost as much as a real check.
pub static DUMMY_VERIFICATIONS: AtomicU64 = AtomicU64::new(0);

/// A hash of a random password nobody knows, made once with the same parameters.
fn dummy_hash() -> &'static str {
    static DUMMY: OnceLock<String> = OnceLock::new();
    DUMMY.get_or_init(|| {
        let mut secret = [0u8; 32];
        let _ = getrandom::fill(&mut secret);
        argon2()
            .hash_password(&secret)
            .map(|h| h.to_string())
            .unwrap_or_default()
    })
}

/// Hashes a new password (PHC string) on the blocking pool.
pub async fn hash_password(password: String) -> Result<String, ApiError> {
    tokio::task::spawn_blocking(move || {
        argon2()
            .hash_password(password.as_bytes())
            .map(|h| h.to_string())
    })
    .await
    .map_err(|e| internal(format!("password hashing stopped: {e}")))?
    .map_err(|e| internal(format!("password hashing failed: {e}")))
}

/// Does `password` match `stored`? With no stored hash, verifies against the dummy hash (and
/// answers no), so every path costs one Argon2id verification.
pub async fn verify_password(password: String, stored: Option<String>) -> bool {
    tokio::task::spawn_blocking(move || {
        let real = stored.is_some();
        let phc = stored.unwrap_or_else(|| {
            DUMMY_VERIFICATIONS.fetch_add(1, Ordering::Relaxed);
            dummy_hash().to_owned()
        });
        let input: &[u8] = if password.len() > MAX_PASSWORD_BYTES {
            b"-"
        } else {
            password.as_bytes()
        };
        let matches = PasswordHash::new(&phc)
            .map(|h| argon2().verify_password(input, &h).is_ok())
            .unwrap_or(false);
        real && matches && password.len() <= MAX_PASSWORD_BYTES
    })
    .await
    .unwrap_or(false)
}

/// The password policy (ADR 0007): length, not composition; not the username.
pub fn check_password(password: &str, username: &str) -> Result<(), String> {
    let n = password.chars().count();
    if !(MIN_PASSWORD..=MAX_PASSWORD).contains(&n) {
        return Err(PASSWORD_RULE.into());
    }
    if password.trim().to_lowercase() == username.to_lowercase() {
        return Err(PASSWORD_RULE.into());
    }
    Ok(())
}

/// A username as stored: trimmed, lowercase, 2 to 64 of `a-z 0-9 . _ -`.
pub fn normalize_username(raw: &str) -> Option<String> {
    let u = raw.trim().to_lowercase();
    let ok = (2..=64).contains(&u.len())
        && u.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')
        });
    ok.then_some(u)
}

/// Why a password sign-in cannot succeed before any password is compared, or the hash to
/// compare with. Pure, so the "always verify something" rule is tested directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// Compare with the account's hash.
    Verify(String),
    /// Verify against the dummy hash, then refuse for this reason.
    Refuse(&'static str),
}

pub fn plan(login: Option<&LocalLogin>) -> Plan {
    match login {
        None => Plan::Refuse("unknown"),
        Some(l) if l.disabled => Plan::Refuse("disabled"),
        Some(l) if l.locked => Plan::Refuse("locked"),
        // Created but the setup link is unused: no password can match yet.
        Some(l) => match &l.password_hash {
            Some(h) => Plan::Verify(h.clone()),
            None => Plan::Refuse("wrong"),
        },
    }
}

// ---- attempts per client address ------------------------------------------------------------------

/// Sign-in attempts per client address in the last minute (this process's memory). The client
/// address is the one the rate limits use: behind a proxy listed in `APELORA_TRUSTED_PROXIES`,
/// the address it forwarded for; IPv6 clients are counted per /64.
pub struct Attempts {
    window: Duration,
    max: usize,
    seen: Mutex<HashMap<IpAddr, VecDeque<Instant>>>,
}

impl Default for Attempts {
    fn default() -> Self {
        Self::new(ATTEMPTS_PER_MINUTE, Duration::from_secs(60))
    }
}

impl Attempts {
    pub fn new(max: usize, window: Duration) -> Self {
        Self {
            window,
            max,
            seen: Mutex::default(),
        }
    }

    /// Records an attempt; `Err(seconds to wait)` when the address has used its allowance.
    pub fn check(&self, ip: IpAddr, now: Instant) -> Result<(), u64> {
        let mut seen = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        if seen.len() > 10_000 {
            seen.retain(|_, q| {
                q.back()
                    .is_some_and(|t| now.duration_since(*t) < self.window)
            });
        }
        let q = seen.entry(ip).or_default();
        while q
            .front()
            .is_some_and(|t| now.duration_since(*t) >= self.window)
        {
            q.pop_front();
        }
        if q.len() >= self.max {
            let wait = q.front().map_or(self.window, |t| {
                self.window.saturating_sub(now.duration_since(*t))
            });
            return Err(wait.as_secs().max(1));
        }
        q.push_back(now);
        Ok(())
    }
}

fn address(client: &ClientAddress) -> IpAddr {
    client.0.unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED))
}

/// 429 `rate_limited` with `Retry-After` when the address has tried too often.
fn limit(state: &AppState, ip: IpAddr) -> Option<Response> {
    let wait = state
        .sign_in_attempts
        .check(super::rate_limit::bucket_key(ip), Instant::now())
        .err()?;
    tracing::warn!(address = %ip, retry_after = wait, "sign-in attempts limited for this address");
    let mut res = ApiError::new(
        StatusCode::TOO_MANY_REQUESTS,
        "rate_limited",
        format!("Too many attempts from this address. Try again in {wait} seconds."),
    )
    .into_response();
    if let Ok(v) = HeaderValue::from_str(&wait.to_string()) {
        res.headers_mut().insert(header::RETRY_AFTER, v);
    }
    Some(res)
}

// ---- helpers --------------------------------------------------------------------------------------

fn internal(message: String) -> ApiError {
    tracing::error!(error = %message, "sign-in: internal failure");
    ApiError::new(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal",
        "the request could not be completed; the server log has the details",
    )
}

fn invalid_credentials() -> ApiError {
    ApiError::new(
        StatusCode::UNAUTHORIZED,
        "invalid_credentials",
        INVALID_CREDENTIALS,
    )
}

fn weak(message: String) -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, "weak_password", message)
}

fn setup_invalid() -> ApiError {
    ApiError::new(
        StatusCode::BAD_REQUEST,
        "setup_invalid",
        "This setup link has been used, has expired or is not known. Ask an administrator for a new one.",
    )
}

/// A fixed message for unreadable bodies: serde's own text could repeat a value from the body.
fn unreadable(_: JsonRejection, fields: &str) -> ApiError {
    ApiError::new(
        StatusCode::BAD_REQUEST,
        "invalid_request",
        format!("the request body must be JSON with {fields}"),
    )
}

/// Responses that carry a secret are never stored by a browser or proxy.
fn no_store(mut res: Response) -> Response {
    res.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

fn rfc3339(t: OffsetDateTime) -> String {
    t.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

fn target(username: &str) -> String {
    format!("local-account:{username}")
}

/// An audit event for someone who is not signed in (yet).
async fn audit_anonymous(
    db: impl apelora_persistence::PgExecutor<'_>,
    user: Option<Uuid>,
    action: &str,
    target: &str,
    outcome: &str,
    detail: Value,
) -> apelora_persistence::Result<()> {
    let mut detail = detail;
    super::requests::tag(&mut detail);
    let actor = user.map_or_else(|| "anonymous".to_owned(), |id| format!("user:{id}"));
    idb::audit_event(
        db,
        &idb::AuditEntry {
            actor: &actor,
            actor_type: if user.is_some() { "user" } else { "anonymous" },
            actor_name: None,
            action,
            target,
            outcome,
            scope: None,
            detail: &detail,
        },
    )
    .await
}

/// A directory sign-in: bind as the person, create or refresh them, then an Apelora session
/// whose external groups are the directory's groups. Each step audits its own outcome.
async fn directory_sign_in(
    state: &AppState,
    pool: &Pool,
    directory: Uuid,
    username: &str,
    password: &str,
) -> Result<PasswordSignInResult, ApiError> {
    let who =
        super::directories::verify_directory_sign_in(state, directory, username, password).await?;
    let mut tx = begin(pool).await?;
    let user = super::directories::upsert_directory_user_in(&mut tx, state, &who).await?;
    let session = issue_session(&mut tx, user.id, SessionMethod::Directory, &who.groups).await?;
    commit(tx).await?;
    tracing::info!(username = %who.username, directory = %who.directory_name, "directory sign-in succeeded");
    Ok(PasswordSignInResult {
        token: session.token,
        expires_at: rfc3339(session.expires_at),
        name: user.display_name.or(user.username).unwrap_or(who.username),
    })
}

// ---- POST /sign-in/password ---------------------------------------------------------------------

#[derive(Deserialize, ToSchema)]
pub struct PasswordSignInRequest {
    pub username: String,
    pub password: String,
    /// Sign in against this directory instead of a local account.
    #[serde(default)]
    pub directory: Option<Uuid>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PasswordSignInResult {
    /// The session token (`apu_…`), sent as `Authorization: Bearer`. Shown only here.
    pub token: String,
    /// RFC 3339; the session also ends after an hour without use.
    pub expires_at: String,
    /// The person's display name, or their username.
    pub name: String,
}

/// Public. Signs a person in with a username and password and returns an Apelora session.
/// Every refusal answers `invalid_credentials` (unknown, wrong, locked or disabled alike).
/// Limited to 10 attempts a minute per client address.
#[utoipa::path(post, path = "/api/v1/sign-in/password", tag = "system", security(()), operation_id = "sign_in_with_password",
    request_body = PasswordSignInRequest,
    responses((status = 200, body = PasswordSignInResult), (status = 400, body = ErrorBody),
        (status = 401, body = ErrorBody), (status = 429, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn sign_in_password(
    State(state): State<AppState>,
    client: ClientAddress,
    body: Result<Json<PasswordSignInRequest>, JsonRejection>,
) -> Response {
    let ip = address(&client);
    if let Some(r) = limit(&state, ip) {
        return r;
    }
    match password_sign_in(&state, ip, body).await {
        Ok(v) => no_store(Json(v).into_response()),
        Err(e) => e.into_response(),
    }
}

async fn password_sign_in(
    state: &AppState,
    ip: IpAddr,
    body: Result<Json<PasswordSignInRequest>, JsonRejection>,
) -> Result<PasswordSignInResult, ApiError> {
    let Json(req) = body.map_err(|e| unreadable(e, "username and password"))?;
    let pool = store(state)?;
    if let Some(directory) = req.directory {
        return directory_sign_in(state, pool, directory, &req.username, &req.password).await;
    }
    if !state.local_accounts_enabled() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "local_accounts_disabled",
            "sign-in with a username and password is turned off on this control plane",
        ));
    }
    let username = normalize_username(&req.username);
    let login = match &username {
        Some(u) => db::login_by_username(pool, u).await?,
        None => None,
    };
    let shown = username
        .clone()
        .unwrap_or_else(|| "(not a valid username)".into());
    let address = ip.to_string();
    let plan = plan(login.as_ref());
    let stored = match &plan {
        Plan::Verify(h) => Some(h.clone()),
        Plan::Refuse(_) => None,
    };
    let matches = verify_password(req.password, stored).await;
    let user_id = login.as_ref().map(|l| l.user_id);
    let refuse = |reason: &'static str| async move {
        tracing::info!(username = %shown, reason, "password sign-in refused");
        audit_failed(
            "auth.password.failed",
            audit_anonymous(
                pool,
                user_id,
                "auth.password.failed",
                &target(&shown),
                "denied",
                json!({ "username": shown, "reason": reason, "address": address }),
            )
            .await,
        );
        invalid_credentials()
    };
    let login = match (plan, matches, login) {
        (Plan::Refuse(reason), _, _) => return Err(refuse(reason).await),
        (Plan::Verify(_), true, Some(l)) => l,
        (Plan::Verify(_), _, l) => {
            if let Some(l) = l
                && db::record_failure(pool, l.user_id).await?
            {
                tracing::warn!(username = %l.username, "local account locked after repeated failures");
                audit_failed(
                    "auth.locked",
                    audit_anonymous(
                        pool,
                        Some(l.user_id),
                        "auth.locked",
                        &target(&l.username),
                        "denied",
                        json!({ "username": l.username, "minutes": db::LOCK_MINUTES, "address": ip.to_string() }),
                    )
                    .await,
                );
            }
            return Err(refuse("wrong").await);
        }
    };
    let mut tx = begin(pool).await?;
    db::record_success_in(&mut tx, login.user_id).await?;
    let session = issue_session(&mut tx, login.user_id, SessionMethod::Password, &[]).await?;
    audit_anonymous(
        &mut *tx,
        Some(login.user_id),
        "auth.password.succeeded",
        &target(&login.username),
        "success",
        json!({ "username": login.username, "address": ip.to_string() }),
    )
    .await?;
    commit(tx).await?;
    tracing::info!(username = %login.username, "password sign-in succeeded");
    Ok(PasswordSignInResult {
        token: session.token,
        expires_at: rfc3339(session.expires_at),
        name: login.display_name.unwrap_or(login.username),
    })
}

// ---- POST /sign-in/setup ------------------------------------------------------------------------

#[derive(Deserialize, ToSchema)]
pub struct PasswordSetupRequest {
    /// The token from the setup link.
    pub token: String,
    pub password: String,
}

/// Public. Sets a local account's password with its one-time setup link (24 hours, single
/// use). `setup_invalid` when the link was used, expired or is unknown; `weak_password` with
/// the rule when the password does not follow it.
#[utoipa::path(post, path = "/api/v1/sign-in/setup", tag = "system", security(()), operation_id = "complete_password_setup",
    request_body = PasswordSetupRequest,
    responses((status = 204), (status = 400, body = ErrorBody), (status = 429, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn complete_setup(
    State(state): State<AppState>,
    client: ClientAddress,
    body: Result<Json<PasswordSetupRequest>, JsonRejection>,
) -> Response {
    let ip = address(&client);
    if let Some(r) = limit(&state, ip) {
        return r;
    }
    match setup(&state, ip, body).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => e.into_response(),
    }
}

async fn setup(
    state: &AppState,
    ip: IpAddr,
    body: Result<Json<PasswordSetupRequest>, JsonRejection>,
) -> Result<(), ApiError> {
    let Json(req) = body.map_err(|e| unreadable(e, "token and password"))?;
    let pool = store(state)?;
    let Some(account) = db::setup_target(pool, req.token.trim()).await? else {
        tracing::info!("setup link refused: used, expired or unknown");
        return Err(setup_invalid());
    };
    check_password(&req.password, &account.username).map_err(weak)?;
    let hashed = hash_password(req.password).await?;
    let mut tx = begin(pool).await?;
    let Some(user) = db::complete_setup_in(&mut tx, req.token.trim(), &hashed).await? else {
        return Err(setup_invalid());
    };
    audit_anonymous(
        &mut *tx,
        Some(user),
        "auth.setup.completed",
        &target(&account.username),
        "success",
        json!({ "username": account.username, "address": ip.to_string() }),
    )
    .await?;
    commit(tx).await?;
    tracing::info!(username = %account.username, "local account password set");
    Ok(())
}

// ---- POST /sign-out -------------------------------------------------------------------------------

/// Ends the Apelora session the request was made with. With any other credential (an
/// identity-provider or API token) there is nothing to end here: 204.
#[utoipa::path(post, path = "/api/v1/sign-out", tag = "system", operation_id = "sign_out",
    responses((status = 204), (status = 401, body = ErrorBody)))]
pub(super) async fn sign_out(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<StatusCode, ApiError> {
    let Some(session) = p.session else {
        return Ok(StatusCode::NO_CONTENT);
    };
    let pool = store(&state)?;
    let mut tx = begin(pool).await?;
    user_sessions::revoke_session(&mut *tx, session.id).await?;
    audit_as(
        &mut tx,
        &p,
        "auth.signed_out",
        &format!("session:{}", session.id),
        &json!({ "method": session.method.as_str() }),
    )
    .await?;
    commit(tx).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---- PUT /me/password -----------------------------------------------------------------------------

#[derive(Deserialize, ToSchema)]
pub struct PasswordChangeRequest {
    pub current: String,
    pub new: String,
}

/// Changes the caller's own password (local accounts only). Every other session of theirs is
/// revoked; the one making the request stays. Counts towards the account's lock like a sign-in.
#[utoipa::path(put, path = "/api/v1/me/password", tag = "system", operation_id = "change_password",
    request_body = PasswordChangeRequest,
    responses((status = 204), (status = 400, body = ErrorBody), (status = 401, body = ErrorBody),
        (status = 409, body = ErrorBody), (status = 429, body = ErrorBody)))]
pub(super) async fn change_password(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    client: ClientAddress,
    body: Result<Json<PasswordChangeRequest>, JsonRejection>,
) -> Response {
    let ip = address(&client);
    if let Some(r) = limit(&state, ip) {
        return r;
    }
    match change(&state, &p, ip, body).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => e.into_response(),
    }
}

async fn change(
    state: &AppState,
    p: &Principal,
    ip: IpAddr,
    body: Result<Json<PasswordChangeRequest>, JsonRejection>,
) -> Result<(), ApiError> {
    let Json(req) = body.map_err(|e| unreadable(e, "current and new"))?;
    let pool = store(state)?;
    let not_local = || {
        ApiError::new(
            StatusCode::CONFLICT,
            "not_a_local_account",
            "only local accounts have a password in Apelora; change yours where you sign in",
        )
    };
    let user = match &p.caller {
        Some(c) if c.kind == CallerKind::User => c.id,
        _ => return Err(not_local()),
    };
    let login = db::login_by_id(pool, user).await?.ok_or_else(not_local)?;
    if login.locked {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "account_locked",
            format!(
                "too many wrong passwords: try again in {} minutes",
                db::LOCK_MINUTES
            ),
        ));
    }
    let same = req.current == req.new;
    if !verify_password(req.current, login.password_hash.clone()).await {
        let locked = db::record_failure(pool, user).await?;
        if locked {
            audit_failed(
                "auth.locked",
                audit_alone(
                    pool,
                    p,
                    "auth.locked",
                    &target(&login.username),
                    "success",
                    None,
                    &json!({ "username": login.username, "minutes": db::LOCK_MINUTES, "address": ip.to_string() }),
                )
                .await,
            );
        }
        audit_failed(
            "auth.password.failed",
            audit_alone(
                pool,
                p,
                "auth.password.failed",
                &target(&login.username),
                "denied",
                None,
                &json!({ "username": login.username, "reason": "wrong", "during": "password change", "address": ip.to_string() }),
            )
            .await,
        );
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "wrong_password",
            "The current password is incorrect.",
        ));
    }
    check_password(&req.new, &login.username).map_err(weak)?;
    if same {
        return Err(weak(
            "Choose a password different from the current one.".into(),
        ));
    }
    let hashed = hash_password(req.new).await?;
    let mut tx = begin(pool).await?;
    let revoked = db::change_password_in(&mut tx, user, &hashed, p.session.map(|s| s.id)).await?;
    audit_as(
        &mut tx,
        p,
        "auth.password.changed",
        &target(&login.username),
        &json!({ "username": login.username, "revokedSessions": revoked }),
    )
    .await?;
    commit(tx).await?;
    Ok(())
}

// ---- local accounts (administration) ---------------------------------------------------------

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LocalAccountInput {
    /// 2 to 64 of `a-z 0-9 . _ -` (stored lowercase).
    pub username: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LocalAccountCreated {
    pub user: UserDto,
    /// The one-time setup token; put it in the link `#/set-password?token=…`. Shown only here.
    pub setup_token: String,
    /// RFC 3339.
    pub setup_expires_at: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LocalAccountSetupLink {
    /// The one-time setup token. Shown only here.
    pub setup_token: String,
    /// RFC 3339.
    pub setup_expires_at: String,
}

fn optional_text(v: Option<&str>, max: usize, what: &str) -> Result<Option<String>, ApiError> {
    let Some(v) = v.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    if v.chars().count() > max || v.chars().any(char::is_control) {
        return Err(super::deploy::bad_request(format!(
            "the {what} is at most {max} characters"
        )));
    }
    Ok(Some(v.to_owned()))
}

/// Creates a local account and returns its one-time setup link, valid for 24 hours. The
/// person chooses their password with it; nobody else ever knows it. Needs `user.manage`.
#[utoipa::path(post, path = "/api/v1/identity/local-accounts", tag = "identity", operation_id = "create_local_account",
    request_body = LocalAccountInput,
    responses((status = 201, body = LocalAccountCreated), (status = 400, body = ErrorBody),
        (status = 403, body = ErrorBody), (status = 409, body = ErrorBody)))]
pub(super) async fn create_local_account(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    headers: HeaderMap,
    body: Result<Json<LocalAccountInput>, JsonRejection>,
) -> Result<Response, ApiError> {
    allow_platform(&p, "user.manage")?;
    let Json(req) = body.map_err(|e| super::deploy::bad_request(e.body_text()))?;
    let username = normalize_username(&req.username).ok_or_else(|| {
        super::deploy::bad_request(
            "a username is 2 to 64 characters: lowercase letters, digits, dots, underscores and hyphens",
        )
    })?;
    let display_name = optional_text(req.display_name.as_deref(), 80, "display name")?;
    let email = optional_text(req.email.as_deref(), 254, "e-mail address")?;
    if let Some(e) = &email
        && (e.matches('@').count() != 1
            || e.contains(char::is_whitespace)
            || e.starts_with('@')
            || e.ends_with('@'))
    {
        return Err(super::deploy::bad_request(
            "the e-mail address is not valid",
        ));
    }
    let pool = store(&state)?;
    let approval = guard(&state, &p, &headers, "user.manage", &target(&username)).await?;
    let mut tx = begin(pool).await?;
    let (user, link) = db::create_local_account_in(
        &mut tx,
        &username,
        display_name.as_deref(),
        email.as_deref(),
        &p.actor_ref(),
    )
    .await
    .map_err(|e| match e {
        StoreError::Conflict(m) => ApiError::new(StatusCode::CONFLICT, "username_taken", m),
        other => other.into(),
    })?;
    audit_as(
        &mut tx,
        &p,
        "identity.local_account.created",
        &target(&username),
        &json!({ "id": user.id, "username": username, "displayName": display_name, "email": email,
                 "setupExpiresAt": rfc3339(link.expires_at), "approval": approval }),
    )
    .await?;
    commit(tx).await?;
    Ok(no_store(
        (
            StatusCode::CREATED,
            Json(LocalAccountCreated {
                user: user_dto(user),
                setup_token: link.token,
                setup_expires_at: rfc3339(link.expires_at),
            }),
        )
            .into_response(),
    ))
}

/// Takes a local account's password away, revokes every session of theirs and returns a new
/// one-time setup link (24 hours). Not for your own account (use Change password). Needs
/// `user.manage`.
#[utoipa::path(post, path = "/api/v1/identity/local-accounts/{id}/reset", tag = "identity", operation_id = "reset_local_account",
    params(("id" = Uuid, Path, description = "User id of the local account")),
    responses((status = 200, body = LocalAccountSetupLink), (status = 403, body = ErrorBody),
        (status = 404, body = ErrorBody), (status = 409, body = ErrorBody)))]
pub(super) async fn reset_local_account(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    allow_platform(&p, "user.manage")?;
    if p.caller.as_ref().is_some_and(|c| c.id == id) {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "self_reset",
            "you cannot reset your own account; use Change password instead",
        ));
    }
    let pool = store(&state)?;
    let login = db::login_by_id(pool, id).await?.ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "not found: local account",
        )
    })?;
    let approval = guard(
        &state,
        &p,
        &headers,
        "user.manage",
        &target(&login.username),
    )
    .await?;
    let mut tx = begin(pool).await?;
    let (link, revoked) = db::reset_local_account_in(&mut tx, id).await?;
    audit_as(
        &mut tx,
        &p,
        "identity.local_account.reset",
        &target(&login.username),
        &json!({ "id": id, "username": login.username, "revokedSessions": revoked,
                 "setupExpiresAt": rfc3339(link.expires_at), "approval": approval }),
    )
    .await?;
    commit(tx).await?;
    Ok(no_store(
        Json(LocalAccountSetupLink {
            setup_token: link.token,
            setup_expires_at: rfc3339(link.expires_at),
        })
        .into_response(),
    ))
}

// ---- the local-accounts setting -----------------------------------------------------------------

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LocalAccountSetting {
    /// Whether people may sign in with a local account (default off).
    pub enabled: bool,
    /// Local accounts with a password that are not disabled: they can sign in.
    pub ready: i64,
    /// Local accounts whose setup link has not been used yet.
    pub pending: i64,
}

#[derive(Deserialize, ToSchema)]
pub struct LocalAccountsRequest {
    pub enabled: bool,
}

/// The local-accounts setting with the counts the Console shows next to it.
pub(super) async fn local_setting(state: &AppState) -> Result<LocalAccountSetting, ApiError> {
    let super::Auth::Tokens(pool) = &state.auth else {
        return Ok(LocalAccountSetting {
            enabled: false,
            ready: 0,
            pending: 0,
        });
    };
    Ok(LocalAccountSetting {
        enabled: state.local_accounts_enabled(),
        ready: db::ready_count(pool).await?,
        pending: db::pending_count(pool).await?,
    })
}

/// Turns sign-in with local accounts on or off. Turning it off is refused while nothing else
/// would let people sign in, and for someone signed in with a local account. Turning it off
/// also ends every password session. Needs `platform.security.manage`; audited.
#[utoipa::path(put, path = "/api/v1/identity/sign-in/local-accounts", tag = "identity", operation_id = "set_local_accounts",
    request_body = LocalAccountsRequest,
    responses((status = 200, body = LocalAccountSetting), (status = 400, body = ErrorBody),
        (status = 403, body = ErrorBody), (status = 409, body = ErrorBody)))]
pub(super) async fn set_local_accounts(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    headers: HeaderMap,
    body: Result<Json<LocalAccountsRequest>, JsonRejection>,
) -> Result<Json<LocalAccountSetting>, ApiError> {
    const MANAGE: &str = "platform.security.manage";
    allow_platform(&p, MANAGE)?;
    let Json(req) = body.map_err(|e| super::deploy::bad_request(e.body_text()))?;
    let pool = store(&state)?;
    if !req.enabled {
        if p.session
            .is_some_and(|s| s.method == SessionMethod::Password)
        {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "would_lock_out",
                "you signed in with a local account: sign in another way first, then turn local accounts off",
            ));
        }
        super::auth_config::guard_leaving_local_accounts(&state)?;
    }
    let approval = guard(&state, &p, &headers, MANAGE, "local-accounts").await?;
    let mut tx = begin(pool).await?;
    apelora_persistence::auth_settings::set_setting(
        &mut *tx,
        LOCAL_ACCOUNTS,
        &json!(req.enabled),
        &p.actor_ref(),
    )
    .await?;
    audit_as(
        &mut tx,
        &p,
        if req.enabled {
            "identity.local_accounts.enabled"
        } else {
            "identity.local_accounts.disabled"
        },
        "local-accounts",
        &json!({ "approval": approval }),
    )
    .await?;
    commit(tx).await?;
    super::auth_config::reload(&state).await?;
    Ok(Json(local_setting(&state).await?))
}

/// Whether local accounts are a way in right now: turned on, with at least one account that
/// has a password and is not disabled. The lockout guards count this.
pub(super) async fn local_way_in(state: &AppState) -> Result<bool, ApiError> {
    let super::Auth::Tokens(pool) = &state.auth else {
        return Ok(false);
    };
    Ok(state.local_accounts_enabled() && db::ready_count(pool).await? > 0)
}

#[cfg(test)]
mod tests;
