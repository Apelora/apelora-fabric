// SPDX-License-Identifier: AGPL-3.0-only
//! Directories people sign in with over LDAP with TLS (ADR 0007, `docs/design/sign-in-methods.md`).
//!
//! Administrators add, check, change, turn off and remove directories under Identity & Access →
//! Sign-in (`platform.security.manage`, audited as `identity.directory.saved|deleted`). Apelora
//! binds as the person with the password they type, over `ldaps://` or StartTLS only, and reads
//! their entry and groups with that bind: no directory password is stored.
//!
//! Sign-in itself is two calls, made by `POST /api/v1/sign-in/password` when its body names a
//! directory: [`verify_directory_sign_in`] (bind, read; audits failures) and
//! [`upsert_directory_user`] (the person in `users`; audits success). The session is then issued
//! with the directory's group names as the session's external groups.

use std::time::Duration;

use apelora_persistence::directories::{self as db, DirectoryFields, DirectoryRecord};
use apelora_persistence::identity::{AuditEntry, UserRow, audit_event, upsert_user};
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;
use uuid::Uuid;

use super::authz::{allow_platform, audit_as, audit_failed, guard};
use super::deploy::{bad_request, store};
use super::ldap::{self, BindForm, DirectoryProblem, Failure};
use super::{ApiError, AppState, ErrorBody, Principal, requests};

const MANAGE: &str = "platform.security.manage";

// ---- settings ----------------------------------------------------------------------------------

/// The settings of a directory. Empty optional fields take the defaults shown.
#[derive(Debug, Clone, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryInput {
    /// What the sign-in form calls it (1 to 60 characters).
    pub name: String,
    /// `ldaps://host[:port]`, or `ldap://host[:port]` with `startTls`. Plain LDAP is refused.
    pub url: String,
    #[serde(default)]
    pub start_tls: bool,
    /// The certificate authority that signed the directory's certificate (PEM); empty: the
    /// system's trusted roots.
    #[serde(default)]
    pub ca_pem: Option<String>,
    /// The bind name with `{username}`: a DN (`uid={username},ou=people,dc=example,dc=org`) or
    /// a principal name (`{username}@example.org`, which needs `baseDn` and `userFilter`).
    pub bind_template: String,
    /// Where to search for the person's entry after the bind.
    #[serde(default)]
    pub base_dn: Option<String>,
    /// Finds the person's entry under `baseDn`, with `{username}`, e.g.
    /// `(userPrincipalName={username}@example.org)`. Empty: the entry is the bind DN itself.
    #[serde(default)]
    pub user_filter: Option<String>,
    /// The entry's stable id (default `entryUUID`; `objectGUID` is read as hex).
    #[serde(default)]
    pub id_attribute: Option<String>,
    /// Default `uid`.
    #[serde(default)]
    pub username_attribute: Option<String>,
    /// Default `mail`.
    #[serde(default)]
    pub email_attribute: Option<String>,
    /// Default `cn`.
    #[serde(default)]
    pub name_attribute: Option<String>,
    /// An attribute of the person's entry that lists their groups, e.g. `memberOf`.
    #[serde(default)]
    pub group_attribute: Option<String>,
    /// Where to search for groups that name the person.
    #[serde(default)]
    pub group_base_dn: Option<String>,
    /// The group search, with `{dn}` and `{username}` (default `(member={dn})`).
    #[serde(default)]
    pub group_filter: Option<String>,
    /// The group attribute that holds the group's name (default `cn`).
    #[serde(default)]
    pub group_name_attribute: Option<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

/// An attribute description: a name (letter, then letters, digits, hyphens) or a numeric OID.
fn is_attribute(a: &str) -> bool {
    let name = a.len() <= 64
        && a.starts_with(|c: char| c.is_ascii_alphabetic())
        && a.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    let oid = a.len() <= 64
        && !a.is_empty()
        && a.split('.')
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
    name || oid
}

fn tidy(v: &Option<String>) -> Option<String> {
    v.as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn dn_field(v: &Option<String>, what: &str) -> Result<Option<String>, String> {
    match tidy(v) {
        Some(d) if d.len() > 512 || d.chars().any(char::is_control) || !d.contains('=') => Err(
            format!("the {what} must be a DN such as ou=people,dc=example,dc=org"),
        ),
        other => Ok(other),
    }
}

fn filter_field(
    v: &Option<String>,
    what: &str,
    placeholders: &[&str],
) -> Result<Option<String>, String> {
    let Some(f) = tidy(v) else { return Ok(None) };
    if f.len() > 512 || f.chars().any(char::is_control) {
        return Err(format!("the {what} is too long or has control characters"));
    }
    let mut sample = f.clone();
    for p in placeholders {
        sample = sample.replace(p, "x");
    }
    if !f.starts_with('(') || ldap3::parse_filter(&sample).is_err() {
        return Err(format!(
            "the {what} is not a valid LDAP filter, e.g. (uid={{username}})"
        ));
    }
    Ok(Some(f))
}

fn attribute(v: &Option<String>, default: &str, what: &str) -> Result<String, String> {
    let a = tidy(v).unwrap_or_else(|| default.to_owned());
    if is_attribute(&a) {
        Ok(a)
    } else {
        Err(format!(
            "the {what} attribute may hold only letters, digits and hyphens"
        ))
    }
}

/// Checks a directory's settings and fills in the defaults. Pure, so it is tested directly.
pub fn validate(i: &DirectoryInput) -> Result<DirectoryFields, String> {
    let name = i.name.trim();
    if name.is_empty() || name.chars().count() > 60 || name.chars().any(char::is_control) {
        return Err("the name must be 1 to 60 characters".into());
    }
    let raw = i.url.trim();
    let url = url::Url::parse(raw)
        .map_err(|_| "the address must look like ldaps://ldap.example.org".to_owned())?;
    let start_tls = match url.scheme() {
        "ldaps" => false,
        "ldap" if i.start_tls => true,
        "ldap" => {
            return Err(
                "plain LDAP is refused: use ldaps:// or turn on StartTLS for an ldap:// address"
                    .into(),
            );
        }
        _ => return Err("the address must start with ldaps:// or ldap://".into()),
    };
    let Some(host) = url.host().map(|h| h.to_string().to_ascii_lowercase()) else {
        return Err("the address must name a host".into());
    };
    if host.is_empty() || host.contains(['%', ' ']) {
        return Err("the address must name a host".into());
    }
    if !url.username().is_empty()
        || url.password().is_some()
        || !matches!(url.path(), "" | "/")
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(
            "the address is only scheme, host and port, e.g. ldaps://ldap.example.org:636".into(),
        );
    }
    let url = match url.port() {
        Some(port) => format!("{}://{host}:{port}", url.scheme()),
        None => format!("{}://{host}", url.scheme()),
    };

    let ca_pem = tidy(&i.ca_pem);
    if let Some(pem) = &ca_pem {
        if pem.len() > 65_536 {
            return Err("the CA certificate is too long (64 KiB at most)".into());
        }
        ldap::parse_ca(pem)?;
    }

    let bind_template = i.bind_template.trim().to_owned();
    if bind_template.matches(ldap::USERNAME).count() != 1
        || bind_template.len() > 512
        || bind_template.chars().any(char::is_control)
    {
        return Err(
            "the bind name must contain {username} once, e.g. uid={username},ou=people,dc=example,dc=org or {username}@example.org"
                .into(),
        );
    }
    let base_dn = dn_field(&i.base_dn, "base DN")?;
    let user_filter = filter_field(&i.user_filter, "user filter", &[ldap::USERNAME])?;
    if let Some(f) = &user_filter {
        if !f.contains(ldap::USERNAME) {
            return Err("the user filter must contain {username}".into());
        }
        if base_dn.is_none() {
            return Err("a user filter needs a base DN to search under".into());
        }
    }
    if ldap::bind_form(&bind_template) == BindForm::Principal && user_filter.is_none() {
        return Err(
            "a bind name that is not a DN (such as {username}@example.org) needs a base DN and a user filter to find the person's entry"
                .into(),
        );
    }

    let group_attribute = match tidy(&i.group_attribute) {
        Some(a) if !is_attribute(&a) => {
            return Err("the group attribute may hold only letters, digits and hyphens".into());
        }
        other => other,
    };
    let group_base_dn = dn_field(&i.group_base_dn, "group base DN")?;
    let group_filter = filter_field(&i.group_filter, "group filter", &[ldap::DN, ldap::USERNAME])?;
    let group_filter = match (&group_base_dn, group_filter) {
        (None, Some(_)) => return Err("a group filter needs a group base DN".into()),
        (Some(_), None) => Some("(member={dn})".to_owned()),
        (_, f) => f,
    };

    Ok(DirectoryFields {
        name: name.to_owned(),
        url,
        start_tls,
        ca_pem,
        bind_template,
        base_dn,
        user_filter,
        id_attribute: attribute(&i.id_attribute, "entryUUID", "id")?,
        username_attribute: attribute(&i.username_attribute, "uid", "username")?,
        email_attribute: attribute(&i.email_attribute, "mail", "email")?,
        name_attribute: attribute(&i.name_attribute, "cn", "name")?,
        group_attribute,
        group_base_dn,
        group_filter,
        group_name_attribute: attribute(&i.group_name_attribute, "cn", "group name")?,
        enabled: i.enabled,
    })
}

/// A saved directory, with whether it answered a TLS connect check just now.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryView {
    pub id: Uuid,
    pub name: String,
    pub url: String,
    pub start_tls: bool,
    pub ca_pem: Option<String>,
    pub bind_template: String,
    pub base_dn: Option<String>,
    pub user_filter: Option<String>,
    pub id_attribute: String,
    pub username_attribute: String,
    pub email_attribute: String,
    pub name_attribute: String,
    pub group_attribute: Option<String>,
    pub group_base_dn: Option<String>,
    pub group_filter: Option<String>,
    pub group_name_attribute: String,
    pub enabled: bool,
    /// Whether a TLS connection (with certificate and host-name checks) worked just now;
    /// false for a directory that is turned off.
    pub reachable: bool,
    pub problem: Option<DirectoryProblem>,
    pub error: Option<String>,
    pub updated_by: String,
}

fn view(r: &DirectoryRecord, reach: Option<&Failure>) -> DirectoryView {
    DirectoryView {
        id: r.id,
        name: r.name.clone(),
        url: r.url.clone(),
        start_tls: r.start_tls,
        ca_pem: r.ca_pem.clone(),
        bind_template: r.bind_template.clone(),
        base_dn: r.base_dn.clone(),
        user_filter: r.user_filter.clone(),
        id_attribute: r.id_attribute.clone(),
        username_attribute: r.username_attribute.clone(),
        email_attribute: r.email_attribute.clone(),
        name_attribute: r.name_attribute.clone(),
        group_attribute: r.group_attribute.clone(),
        group_base_dn: r.group_base_dn.clone(),
        group_filter: r.group_filter.clone(),
        group_name_attribute: r.group_name_attribute.clone(),
        enabled: r.enabled,
        reachable: r.enabled && reach.is_none(),
        problem: reach.map(Failure::problem),
        error: reach.map(Failure::message),
        updated_by: r.updated_by.clone(),
    }
}

async fn reachability(r: &DirectoryRecord, deadline: Duration) -> Option<Failure> {
    if !r.enabled {
        return None;
    }
    let result = ldap::reach(&r.fields(), deadline).await;
    if let Err(e) = &result {
        tracing::warn!(directory = %r.name, url = %r.url, problem = ?e.problem(), error = %e.message(), "directory not reachable");
    }
    result.err()
}

/// Every saved directory with its reachability, checked in parallel.
pub(super) async fn views(state: &AppState) -> Result<Vec<DirectoryView>, ApiError> {
    let super::Auth::Tokens(pool) = &state.auth else {
        return Ok(Vec::new());
    };
    let records = db::list_directories(pool).await?;
    let mut checks = tokio::task::JoinSet::new();
    for (i, r) in records.iter().cloned().enumerate() {
        checks.spawn(async move { (i, reachability(&r, ldap::REACH_DEADLINE).await) });
    }
    let mut results: Vec<Option<Failure>> = vec![None; records.len()];
    while let Some(done) = checks.join_next().await {
        if let Ok((i, failure)) = done {
            results[i] = failure;
        }
    }
    Ok(records
        .iter()
        .zip(&results)
        .map(|(r, f)| view(r, f.as_ref()))
        .collect())
}

/// A directory people can choose on the sign-in page.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryOption {
    pub id: Uuid,
    pub name: String,
}

/// The enabled directories, for the public sign-in options. Never fails: a database problem
/// leaves the list empty (and is logged), so the other ways in are still offered.
pub(super) async fn sign_in_options(state: &AppState) -> Vec<DirectoryOption> {
    let super::Auth::Tokens(pool) = &state.auth else {
        return Vec::new();
    };
    match db::list_directories(pool).await {
        Ok(list) => list
            .into_iter()
            .filter(|d| d.enabled)
            .map(|d| DirectoryOption {
                id: d.id,
                name: d.name,
            })
            .collect(),
        Err(e) => {
            tracing::warn!(error = %e, "sign-in options: directories could not be read");
            Vec::new()
        }
    }
}

#[derive(Serialize, ToSchema)]
pub struct DirectoryList {
    pub items: Vec<DirectoryView>,
}

/// The directories people sign in with, each checked with a TLS connection now. Needs
/// `platform.security.manage`.
#[utoipa::path(get, path = "/api/v1/identity/sign-in/directories", tag = "identity", operation_id = "list_directories",
    responses((status = 200, body = DirectoryList), (status = 401, body = ErrorBody), (status = 403, body = ErrorBody)))]
pub(super) async fn list(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Json<DirectoryList>, ApiError> {
    allow_platform(&p, MANAGE)?;
    Ok(Json(DirectoryList {
        items: views(&state).await?,
    }))
}

fn not_found() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "not_found", "not found: directory")
}

async fn save(
    state: &AppState,
    p: &Principal,
    headers: &HeaderMap,
    id: Option<Uuid>,
    input: DirectoryInput,
) -> Result<DirectoryView, ApiError> {
    allow_platform(p, MANAGE)?;
    let fields = validate(&input).map_err(bad_request)?;
    let pool = store(state)?;
    if let Some(id) = id {
        db::get_directory(pool, id).await?.ok_or_else(not_found)?;
    }
    let approval = guard(
        state,
        p,
        headers,
        MANAGE,
        &format!("directory:{}", fields.url),
    )
    .await?;
    let id = db::save_directory(pool, id, &fields, &p.actor_ref())
        .await
        .map_err(|e| match e {
            apelora_persistence::StoreError::Database(e)
                if e.to_string().contains("duplicate key") =>
            {
                ApiError::new(
                    StatusCode::CONFLICT,
                    "directory_exists",
                    "another directory has this name or address",
                )
            }
            other => other.into(),
        })?;
    let saved = db::get_directory(pool, id).await?.ok_or_else(|| {
        ApiError::new(
            StatusCode::CONFLICT,
            "conflict",
            "the directory was saved, but another change removed it; reload the sign-in settings",
        )
    })?;
    let reach = reachability(&saved, ldap::REACH_DEADLINE).await;
    audit_as(pool, p, "identity.directory.saved", &format!("directory:{id}"),
        &json!({ "id": id, "name": fields.name, "url": fields.url, "startTls": fields.start_tls, "enabled": fields.enabled, "reachable": saved.enabled && reach.is_none(), "approval": approval }))
        .await?;
    Ok(view(&saved, reach.as_ref()))
}

/// Adds a directory; it is offered on the sign-in page at once when enabled. Needs
/// `platform.security.manage` (and an approval where the policy asks for one); audited.
#[utoipa::path(post, path = "/api/v1/identity/sign-in/directories", tag = "identity", operation_id = "create_directory",
    request_body = DirectoryInput,
    responses((status = 201, body = DirectoryView), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody), (status = 409, body = ErrorBody)))]
pub(super) async fn create(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    headers: HeaderMap,
    body: Result<Json<DirectoryInput>, JsonRejection>,
) -> Result<(StatusCode, Json<DirectoryView>), ApiError> {
    let Json(input) = body.map_err(|e| bad_request(e.body_text()))?;
    Ok((
        StatusCode::CREATED,
        Json(save(&state, &p, &headers, None, input).await?),
    ))
}

/// Changes a directory (including turning it off). Needs `platform.security.manage`; audited.
#[utoipa::path(put, path = "/api/v1/identity/sign-in/directories/{id}", tag = "identity", operation_id = "update_directory",
    params(("id" = Uuid, Path, description = "Directory id")), request_body = DirectoryInput,
    responses((status = 200, body = DirectoryView), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody)))]
pub(super) async fn update(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    body: Result<Json<DirectoryInput>, JsonRejection>,
) -> Result<Json<DirectoryView>, ApiError> {
    let Json(input) = body.map_err(|e| bad_request(e.body_text()))?;
    Ok(Json(save(&state, &p, &headers, Some(id), input).await?))
}

#[derive(Deserialize, ToSchema)]
pub struct ConfirmDirectory {
    /// The directory's name again.
    pub confirm: String,
}

/// Removes a directory: its people can no longer sign in with it; their accounts and roles in
/// Apelora stay. Its name must be typed again. Needs `platform.security.manage`; audited.
#[utoipa::path(delete, path = "/api/v1/identity/sign-in/directories/{id}", tag = "identity", operation_id = "delete_directory",
    params(("id" = Uuid, Path, description = "Directory id")), request_body = ConfirmDirectory,
    responses((status = 204), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody)))]
pub(super) async fn delete(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    body: Result<Json<ConfirmDirectory>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    allow_platform(&p, MANAGE)?;
    let Json(req) = body.map_err(|e| bad_request(e.body_text()))?;
    let pool = store(&state)?;
    let existing = db::get_directory(pool, id).await?.ok_or_else(not_found)?;
    if req.confirm != existing.name {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "confirmation_mismatch",
            format!("type the directory's name ({}) to confirm", existing.name),
        ));
    }
    let approval = guard(
        &state,
        &p,
        &headers,
        MANAGE,
        &format!("directory:{}", existing.url),
    )
    .await?;
    db::delete_directory(pool, id).await?;
    audit_as(
        pool,
        &p,
        "identity.directory.deleted",
        &format!("directory:{id}"),
        &json!({ "id": id, "name": existing.name, "url": existing.url, "approval": approval }),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Settings to check (saved or not) and a test username and password, which are used for one
/// bind and never stored or logged.
#[derive(Deserialize, ToSchema)]
pub struct DirectoryCheckRequest {
    pub settings: DirectoryInput,
    pub username: String,
    #[schema(format = Password)]
    pub password: String,
}

/// What the check found for the test person.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CheckedEntry {
    pub dn: String,
    /// The stable id Apelora would identify the person by.
    pub subject: String,
    pub username: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
    /// Group names from the directory.
    pub groups: Vec<String>,
    /// Apelora groups whose external name matches one of `groups`.
    pub mapped_groups: Vec<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryCheck {
    pub ok: bool,
    /// What went wrong, when it did.
    pub problem: Option<DirectoryProblem>,
    pub message: String,
    pub entry: Option<CheckedEntry>,
}

/// Connects with TLS, binds as a test person and reads their entry and groups, without saving
/// anything. Reports what it found or the precise reason it failed. Needs
/// `platform.security.manage`.
#[utoipa::path(post, path = "/api/v1/identity/sign-in/directories/check", tag = "identity", operation_id = "check_directory",
    request_body = DirectoryCheckRequest,
    responses((status = 200, body = DirectoryCheck), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody)))]
pub(super) async fn check(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    body: Result<Json<DirectoryCheckRequest>, JsonRejection>,
) -> Result<Json<DirectoryCheck>, ApiError> {
    allow_platform(&p, MANAGE)?;
    let Json(req) = body.map_err(|e| bad_request(e.body_text()))?;
    let fields = validate(&req.settings).map_err(bad_request)?;
    let username = req.username.trim();
    if username.is_empty() || req.password.is_empty() {
        return Err(bad_request(
            "type a test username and password: the check signs in as that person",
        ));
    }
    ldap::check_username(username, ldap::bind_form(&fields.bind_template)).map_err(bad_request)?;
    let result = ldap::sign_in(&fields, username, &req.password, ldap::SIGN_IN_DEADLINE).await;
    tracing::info!(directory = %fields.name, url = %fields.url, username, outcome = ?result.as_ref().err().map(Failure::problem), "directory check");
    Ok(Json(match result {
        Ok(person) => {
            let mapped = match &state.auth {
                super::Auth::Tokens(pool) => db::mapped_groups(pool, &person.groups)
                    .await?
                    .into_iter()
                    .map(|(_, name)| name)
                    .collect(),
                super::Auth::DisabledForDevelopment => Vec::new(),
            };
            DirectoryCheck {
                ok: true,
                problem: None,
                message: format!(
                    "signed in as {} and read the entry with {} group{}",
                    person.username,
                    person.groups.len(),
                    if person.groups.len() == 1 { "" } else { "s" }
                ),
                entry: Some(CheckedEntry {
                    dn: person.dn,
                    subject: person.subject,
                    username: person.username,
                    email: person.email,
                    display_name: person.display_name,
                    groups: person.groups,
                    mapped_groups: mapped,
                }),
            }
        }
        Err(f) => DirectoryCheck {
            ok: false,
            problem: Some(f.problem()),
            message: f.message(),
            entry: None,
        },
    }))
}

// ---- sign-in -----------------------------------------------------------------------------------

/// A person the directory vouched for. `groups` are the directory's group names: they become
/// the session's external groups and map onto Apelora groups by external name.
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "called by POST /api/v1/sign-in/password, wired when local-account sessions land (ADR 0007)"
    )
)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryIdentity {
    pub directory_id: Uuid,
    pub directory_name: String,
    /// The directory's URL: the `users.issuer` of its people.
    pub issuer: String,
    /// The entry's stable id: the `users.subject`.
    pub subject: String,
    pub username: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub groups: Vec<String>,
}

/// The answer to a wrong username or password (and to an empty one): the same words as for
/// local accounts, so the form does not reveal which part was wrong.
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "called by POST /api/v1/sign-in/password, wired when local-account sessions land (ADR 0007)"
    )
)]
pub(super) fn invalid_credentials() -> ApiError {
    ApiError::new(
        StatusCode::UNAUTHORIZED,
        "invalid_credentials",
        "The username or password is incorrect.",
    )
}

async fn audit_sign_in(
    state: &AppState,
    actor: (&str, &str),
    outcome: &str,
    action: &str,
    (directory_id, directory_name): (Uuid, &str),
    mut detail: serde_json::Value,
) {
    let super::Auth::Tokens(pool) = &state.auth else {
        return;
    };
    if let Some(o) = detail.as_object_mut() {
        o.insert("directory".into(), json!(directory_name));
    }
    requests::tag(&mut detail);
    audit_failed(
        action,
        audit_event(
            pool,
            &AuditEntry {
                actor: actor.0,
                actor_type: actor.1,
                action,
                target: &format!("directory:{directory_id}"),
                outcome,
                scope: None,
                detail: &detail,
            },
        )
        .await,
    );
}

/// Binds to the directory as the person and reads who they are, within 10 seconds. Failures
/// are audited as `auth.directory.failed` with the username and a reason, never the password:
/// a refused bind (or an empty password) is `401 invalid_credentials`; a directory that cannot
/// be reached, or whose certificate fails its checks, is `503 directory_unreachable`; an entry
/// that cannot be read after a good bind is `502 directory_entry_unreadable`.
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "called by POST /api/v1/sign-in/password, wired when local-account sessions land (ADR 0007)"
    )
)]
pub async fn verify_directory_sign_in(
    state: &AppState,
    directory_id: Uuid,
    username: &str,
    password: &str,
) -> Result<DirectoryIdentity, ApiError> {
    verify_within(
        state,
        directory_id,
        username,
        password,
        ldap::SIGN_IN_DEADLINE,
    )
    .await
}

pub(super) async fn verify_within(
    state: &AppState,
    directory_id: Uuid,
    username: &str,
    password: &str,
    deadline: Duration,
) -> Result<DirectoryIdentity, ApiError> {
    let pool = store(state)?;
    let directory = db::get_directory(pool, directory_id)
        .await?
        .filter(|d| d.enabled)
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::BAD_REQUEST,
                "directory_unavailable",
                "this directory is not offered for sign-in; reload the page",
            )
        })?;
    let username = username.trim();
    match ldap::sign_in(&directory.fields(), username, password, deadline).await {
        Ok(person) => Ok(DirectoryIdentity {
            directory_id,
            directory_name: directory.name,
            issuer: directory.url,
            subject: person.subject,
            username: person.username,
            email: person.email,
            display_name: person.display_name,
            groups: person.groups,
        }),
        Err(failure) => {
            let (reason, error) = match &failure {
                Failure::BindRefused => ("wrong", invalid_credentials()),
                f if f.is_connection() => {
                    tracing::warn!(directory = %directory.name, url = %directory.url, problem = ?f.problem(), error = %f.message(), "directory sign-in: the directory cannot be used");
                    (
                        "unreachable",
                        ApiError::new(
                            StatusCode::SERVICE_UNAVAILABLE,
                            "directory_unreachable",
                            format!(
                                "The directory {} cannot be reached right now. Try again later or sign in another way.",
                                directory.name
                            ),
                        ),
                    )
                }
                f => {
                    tracing::warn!(directory = %directory.name, url = %directory.url, problem = ?f.problem(), error = %f.message(), "directory sign-in: the person's entry could not be read");
                    (
                        "entry_unreadable",
                        ApiError::new(
                            StatusCode::BAD_GATEWAY,
                            "directory_entry_unreadable",
                            format!(
                                "The directory {} accepted the password, but your entry could not be read. Ask an administrator to check the directory's settings.",
                                directory.name
                            ),
                        ),
                    )
                }
            };
            audit_sign_in(
                state,
                ("anonymous", "anonymous"),
                "denied",
                "auth.directory.failed",
                (directory.id, &directory.name),
                json!({ "username": username, "reason": reason, "problem": failure.problem() }),
            )
            .await;
            Err(error)
        }
    }
}

/// Creates the person on their first directory sign-in (issuer: the directory's URL, subject:
/// the entry's stable id) and refreshes their profile later. A disabled person is refused
/// (`403 account_disabled`). Success is audited as `auth.directory.succeeded`.
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "called by POST /api/v1/sign-in/password, wired when local-account sessions land (ADR 0007)"
    )
)]
pub async fn upsert_directory_user(
    state: &AppState,
    identity: &DirectoryIdentity,
) -> Result<UserRow, ApiError> {
    let pool = store(state)?;
    let user = upsert_user(
        pool,
        &identity.issuer,
        &identity.subject,
        Some(&identity.username),
        identity.email.as_deref(),
        identity.display_name.as_deref(),
    )
    .await?;
    let directory = (identity.directory_id, identity.directory_name.as_str());
    let actor = format!("user:{}", user.id);
    if user.disabled_at.is_some() {
        audit_sign_in(
            state,
            (&actor, "user"),
            "denied",
            "auth.directory.failed",
            directory,
            json!({ "username": identity.username, "reason": "disabled" }),
        )
        .await;
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "account_disabled",
            "this account has been disabled",
        ));
    }
    audit_sign_in(
        state,
        (&actor, "user"),
        "success",
        "auth.directory.succeeded",
        directory,
        json!({ "username": identity.username, "groups": identity.groups }),
    )
    .await;
    Ok(user)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> DirectoryInput {
        DirectoryInput {
            name: " Lab directory ".into(),
            url: "LDAPS://LDAP.Example.TEST:636/".into(),
            bind_template: "uid={username},ou=people,dc=example,dc=test".into(),
            enabled: true,
            ..DirectoryInput::default()
        }
    }

    #[test]
    fn settings_are_tidied_and_defaults_filled_in() {
        let f = validate(&input()).expect("valid");
        assert_eq!(f.name, "Lab directory");
        assert_eq!(f.url, "ldaps://ldap.example.test:636");
        assert!(!f.start_tls);
        assert_eq!(
            (
                f.id_attribute.as_str(),
                f.username_attribute.as_str(),
                f.email_attribute.as_str(),
                f.name_attribute.as_str(),
                f.group_name_attribute.as_str()
            ),
            ("entryUUID", "uid", "mail", "cn", "cn")
        );
        assert_eq!(
            (f.group_attribute, f.group_base_dn, f.group_filter),
            (None, None, None)
        );

        let mut i = input();
        i.url = "ldap://ldap.example.test".into();
        i.start_tls = true;
        i.group_base_dn = Some("ou=groups,dc=example,dc=test".into());
        i.ca_pem = Some("  ".into());
        let f = validate(&i).expect("StartTLS");
        assert!(f.start_tls);
        assert_eq!(f.ca_pem, None);
        assert_eq!(
            f.group_filter.as_deref(),
            Some("(member={dn})"),
            "default group search"
        );

        let mut i = input();
        i.url = "ldaps://[::1]:1636".into();
        assert_eq!(validate(&i).expect("IPv6").url, "ldaps://[::1]:1636");
        // A principal name with its search.
        let mut i = input();
        i.bind_template = "{username}@example.test".into();
        i.base_dn = Some("dc=example,dc=test".into());
        i.user_filter = Some("(userPrincipalName={username}@example.test)".into());
        i.id_attribute = Some("objectGUID".into());
        i.group_attribute = Some("memberOf".into());
        validate(&i).expect("principal form");
    }

    #[test]
    fn unsafe_or_incomplete_settings_are_refused() {
        let bad = |f: fn(&mut DirectoryInput)| {
            let mut i = input();
            f(&mut i);
            validate(&i).err()
        };
        let plain = bad(|i| i.url = "ldap://ldap.example.test".into()).expect("plain LDAP");
        assert!(plain.contains("plain LDAP is refused"), "{plain}");
        assert!(bad(|i| i.url = "https://ldap.example.test".into()).is_some());
        assert!(bad(|i| i.url = "ldaps://".into()).is_some());
        assert!(
            bad(|i| i.url = "ldaps://user:pw@ldap.example.test".into()).is_some(),
            "no credentials in the URL"
        );
        assert!(bad(|i| i.url = "ldaps://ldap.example.test/dc=x??sub".into()).is_some());
        assert!(bad(|i| i.name = "".into()).is_some());
        assert!(bad(|i| i.name = "x".repeat(61)).is_some());
        assert!(
            bad(|i| i.bind_template = "uid=fixed,dc=x".into()).is_some(),
            "no {{username}}"
        );
        assert!(
            bad(|i| i.bind_template = "uid={username},cn={username}".into()).is_some(),
            "twice"
        );
        assert!(
            bad(|i| i.bind_template = "{username}@example.test".into()).is_some(),
            "a principal name needs a search"
        );
        assert!(
            bad(|i| i.user_filter = Some("(uid={username})".into())).is_some(),
            "needs a base DN"
        );
        assert!(
            bad(|i| {
                i.base_dn = Some("dc=x".into());
                i.user_filter = Some("(uid=fixed)".into());
            })
            .is_some(),
            "the filter must use the username"
        );
        assert!(
            bad(|i| {
                i.base_dn = Some("dc=x".into());
                i.user_filter = Some("uid={username}".into());
            })
            .is_some(),
            "not parenthesised"
        );
        assert!(
            bad(|i| {
                i.base_dn = Some("dc=x".into());
                i.user_filter = Some("(&(uid={username})".into());
            })
            .is_some(),
            "unbalanced"
        );
        assert!(
            bad(|i| i.base_dn = Some("people".into())).is_some(),
            "not a DN"
        );
        assert!(
            bad(|i| i.group_filter = Some("(member={dn})".into())).is_some(),
            "needs a group base"
        );
        for attr in ["mail;binary", "ma il", "1mail", "(mail)", "mail*", ""] {
            let mut i = input();
            i.email_attribute = Some(attr.into());
            assert_eq!(
                validate(&i).is_err(),
                !attr.is_empty(),
                "{attr:?}: empty takes the default"
            );
        }
        let mut i = input();
        i.group_attribute = Some("memberOf)(x".into());
        assert!(validate(&i).is_err());
        let mut i = input();
        i.id_attribute = Some("1.3.6.1.1.16.4".into());
        assert!(validate(&i).is_ok(), "a numeric OID is an attribute");
        assert!(bad(|i| i.ca_pem = Some("not a certificate".into())).is_some());
    }
}
