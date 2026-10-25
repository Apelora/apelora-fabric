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
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;

use super::authz::{allow_platform, guard};
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

pub(super) fn admin(state: &AppState) -> Result<&Provider, ApiError> {
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
pub(super) fn audit_store(state: &AppState) -> Result<&apelora_persistence::Pool, ApiError> {
    match &state.auth {
        Auth::Tokens(pool) => Ok(pool),
        Auth::DisabledForDevelopment => Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "database_not_configured",
            "cluster administration is audited and needs a database (DATABASE_URL)",
        )),
    }
}

pub(super) fn member_name(name: &str) -> Result<(), ApiError> {
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
    headers: HeaderMap,
    body: Result<Json<CreateJoinToken>, JsonRejection>,
) -> Result<Response, ApiError> {
    allow_platform(&p, "cluster.sensitive_change")?;
    let Json(req) =
        body.map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, "invalid_request", e.body_text()))?;
    member_name(&req.member_name)?;
    let pool = audit_store(&state)?;
    let provider = admin(&state)?;
    let approval = guard(
        &state,
        &p,
        &headers,
        "cluster.sensitive_change",
        &format!("join-token:{}", req.member_name),
    )
    .await?;
    let t = provider.create_join_token(&req.member_name).await?;
    super::authz::audit_alone(
        pool,
        &p,
        "cluster.join-token.created",
        &t.member_name,
        "success",
        None,
        &json!({ "role": p.role, "expiresAt": t.expires_at, "approval": approval }),
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
    allow_platform(&p, "cluster.manage")?;
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
    allow_platform(&p, "cluster.manage")?;
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
    super::authz::audit_alone(
        pool,
        &p,
        "cluster.join-token.revoked",
        &member,
        "success",
        None,
        &json!({ "role": p.role }),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---- Evacuation (Phase 08) -------------------------------------------------------------------

#[derive(Serialize, ToSchema)]
pub struct EvacuationPreview {
    pub source: Source,
    pub plan: apelora_domain::evacuation::EvacuationPlan,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConfirmMember {
    /// The member's name again, typed by the administrator.
    pub confirm: String,
    /// Evacuate although the dry run found blockers (a workload may lose service). Needs the
    /// unsafe-evacuation permission and an approval from another person.
    #[serde(default)]
    pub force: bool,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MemberStateStarted {
    pub member: String,
    /// Incus operation carrying out the evacuation or restore.
    pub operation: String,
}

/// The dry run: what evacuating `member` does to each instance on it, with what blocks it
/// and what to accept. Uses the admin identity when configured, so every project is seen.
pub(super) async fn evacuation_plan(
    state: &AppState,
    member: &str,
) -> Result<
    (
        apelora_domain::evacuation::EvacuationPlan,
        apelora_domain::ProviderMode,
    ),
    ApiError,
> {
    use apelora_domain::evacuation::{Deployed, plan_evacuation};
    use apelora_domain::plan::DesiredModule;
    let provider: &Provider = state.admin.as_deref().unwrap_or(&state.provider);
    let members = provider.cluster_members().await?;
    let instances = provider.instances().await?;
    let records = match &state.auth {
        Auth::Tokens(pool) => apelora_persistence::list_deployments(pool).await?,
        Auth::DisabledForDevelopment => Vec::new(),
    };
    let desired: Vec<(String, String, Vec<DesiredModule>)> = records
        .into_iter()
        .filter_map(|r| {
            let desired = serde_json::from_value(r.desired)
                .map_err(|e| tracing::warn!(application = %r.application, error = %e, "evacuation: the deployment record is unreadable; its application is not considered"))
                .ok()?;
            Some((r.application, r.project, desired))
        })
        .collect();
    let deployed: Vec<Deployed<'_>> = desired
        .iter()
        .map(|(application, project, desired)| Deployed {
            application,
            project,
            desired,
        })
        .collect();
    let mut plan = plan_evacuation(member, &members, &instances, &deployed)
        .map_err(|e| ApiError::new(StatusCode::NOT_FOUND, "not_found", e))?;
    if state.admin.is_none() {
        plan.warnings.push(
            "only the deploy projects are visible (no admin identity): instances of other projects are not listed".into(),
        );
    }
    tracing::debug!(member, targets = ?plan.targets, instances = plan.items.len(), blocked = ?plan.blocked, warnings = plan.warnings.len(), "evacuation: plan");
    Ok((plan, provider.mode()))
}

/// Dry run of evacuating a cluster member: per instance, what Incus will do and what it means
/// for the application; what blocks it; what to accept. Changes nothing.
#[utoipa::path(get, path = "/api/v1/cluster/members/{member}/evacuation", tag = "cluster", operation_id = "preview_evacuation",
    params(("member" = String, Path, description = "Cluster member name")),
    responses((status = 200, body = EvacuationPreview), (status = 400, body = ErrorBody),
        (status = 401, body = ErrorBody), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody),
        (status = 502, body = ErrorBody)))]
pub(super) async fn preview_evacuation(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(member): Path<String>,
) -> Result<Json<EvacuationPreview>, ApiError> {
    require(&p, Permission::ReadInventory)?;
    member_name(&member)?;
    let (plan, mode) = evacuation_plan(&state, &member).await?;
    Ok(Json(EvacuationPreview {
        source: source(mode),
        plan,
    }))
}

/// Checks the typed confirmation; returns whether the caller asked to go ahead despite blockers.
fn confirm(
    req: Result<Json<ConfirmMember>, JsonRejection>,
    member: &str,
) -> Result<bool, ApiError> {
    let Json(req) =
        req.map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, "invalid_request", e.body_text()))?;
    if req.confirm != member {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "confirmation_mismatch",
            format!("type the member's name ({member}) to confirm"),
        ));
    }
    Ok(req.force)
}

/// Evacuates a cluster member through Incus after re-running the dry run: refused while
/// anything blocks it. Needs `manage-infrastructure` and the admin identity; audited.
#[utoipa::path(post, path = "/api/v1/cluster/members/{member}/evacuate", tag = "cluster", operation_id = "evacuate_member",
    params(("member" = String, Path, description = "Cluster member name")), request_body = ConfirmMember,
    responses((status = 202, body = MemberStateStarted), (status = 400, body = ErrorBody),
        (status = 401, body = ErrorBody), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody),
        (status = 409, body = ErrorBody), (status = 502, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn evacuate(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    headers: HeaderMap,
    Path(member): Path<String>,
    body: Result<Json<ConfirmMember>, JsonRejection>,
) -> Result<(StatusCode, Json<MemberStateStarted>), ApiError> {
    member_name(&member)?;
    let force = confirm(body, &member)?;
    // Evacuating when nothing blocks it is an ordinary cluster operation. Going ahead although
    // something blocks it (`force`) can leave a workload without service: it needs the
    // unsafe-evacuation permission and another person's approval.
    allow_platform(
        &p,
        if force {
            "cluster.evacuate_unsafe"
        } else {
            "cluster.evacuate"
        },
    )?;
    let pool = audit_store(&state)?;
    let provider = admin(&state)?;
    let (plan, _) = evacuation_plan(&state, &member).await?;
    let unsafe_run = !plan.blocked.is_empty();
    if unsafe_run && !force {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "evacuation_blocked",
            format!("nothing was changed: {}", plan.blocked.join("; ")),
        ));
    }
    let approval = guard(
        &state,
        &p,
        &headers,
        if unsafe_run {
            "cluster.evacuate_unsafe"
        } else {
            "cluster.evacuate"
        },
        &format!("evacuate:{member}"),
    )
    .await?;
    let operation = provider.member_state(&member, false).await?;
    super::authz::audit_alone(pool, &p,
        if unsafe_run { "cluster.member.evacuate.unsafe" } else { "cluster.member.evacuate" },
        &member,
        "success",
        None,
        &json!({ "role": p.role, "operation": operation, "instances": plan.items.len(), "warnings": plan.warnings, "blocked": plan.blocked, "approval": approval }),
    )
    .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(MemberStateStarted { member, operation }),
    ))
}

/// Restores an evacuated member: Incus moves its instances back and starts them. Needs
/// `manage-infrastructure` and the admin identity; audited.
#[utoipa::path(post, path = "/api/v1/cluster/members/{member}/restore", tag = "cluster", operation_id = "restore_member",
    params(("member" = String, Path, description = "Cluster member name")), request_body = ConfirmMember,
    responses((status = 202, body = MemberStateStarted), (status = 400, body = ErrorBody),
        (status = 401, body = ErrorBody), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody),
        (status = 502, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn restore(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(member): Path<String>,
    body: Result<Json<ConfirmMember>, JsonRejection>,
) -> Result<(StatusCode, Json<MemberStateStarted>), ApiError> {
    allow_platform(&p, "cluster.evacuate")?;
    member_name(&member)?;
    confirm(body, &member)?;
    let pool = audit_store(&state)?;
    let provider = admin(&state)?;
    let operation = provider.member_state(&member, true).await?;
    super::authz::audit_alone(
        pool,
        &p,
        "cluster.member.restore",
        &member,
        "success",
        None,
        &json!({ "role": p.role, "operation": operation }),
    )
    .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(MemberStateStarted { member, operation }),
    ))
}

#[derive(Serialize, ToSchema)]
pub struct RemovalPreview {
    pub source: Source,
    pub plan: apelora_domain::removal::RemovalPlan,
}

/// The removal check: what is still on `member` and whether the cluster keeps running without
/// it. Uses the admin identity when configured, so every project is seen.
async fn removal_plan(
    state: &AppState,
    member: &str,
) -> Result<
    (
        apelora_domain::removal::RemovalPlan,
        apelora_domain::ProviderMode,
    ),
    ApiError,
> {
    use apelora_domain::removal::{RecordedVolume, plan_removal};
    let provider: &Provider = state.admin.as_deref().unwrap_or(&state.provider);
    let members = provider.cluster_members().await?;
    let instances = provider.instances().await?;
    let records = match &state.auth {
        Auth::Tokens(pool) => apelora_persistence::all_volumes(pool).await?,
        Auth::DisabledForDevelopment => Vec::new(),
    };
    let volumes: Vec<RecordedVolume<'_>> = records
        .iter()
        .map(|r| RecordedVolume {
            project: &r.project,
            name: &r.incus_name,
            member: &r.member,
        })
        .collect();
    let mut plan = plan_removal(member, &members, &instances, &volumes)
        .map_err(|e| ApiError::new(StatusCode::NOT_FOUND, "not_found", e))?;
    if state.admin.is_none() {
        plan.blocked.push(
            "cluster administration needs the admin identity: instances of other projects cannot be checked".into(),
        );
    }
    tracing::debug!(member, status = %plan.status, instances = ?plan.instances, volumes = ?plan.volumes, blocked = ?plan.blocked, "removal: plan");
    Ok((plan, provider.mode()))
}

/// Dry run of removing a cluster member for good: what is still on it, what blocks it and what
/// to accept. Changes nothing.
#[utoipa::path(get, path = "/api/v1/cluster/members/{member}/removal", tag = "cluster", operation_id = "preview_member_removal",
    params(("member" = String, Path, description = "Cluster member name")),
    responses((status = 200, body = RemovalPreview), (status = 400, body = ErrorBody),
        (status = 401, body = ErrorBody), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody),
        (status = 502, body = ErrorBody)))]
pub(super) async fn preview_removal(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(member): Path<String>,
) -> Result<Json<RemovalPreview>, ApiError> {
    require(&p, Permission::ReadInventory)?;
    member_name(&member)?;
    let (plan, mode) = removal_plan(&state, &member).await?;
    Ok(Json(RemovalPreview {
        source: source(mode),
        plan,
    }))
}

/// Removes an evacuated, empty member from the cluster for good, after the check runs again:
/// refused while anything blocks it. Needs `cluster.manage`, the admin identity and the
/// member's name typed again; audited.
#[utoipa::path(post, path = "/api/v1/cluster/members/{member}/remove", tag = "cluster", operation_id = "remove_member",
    params(("member" = String, Path, description = "Cluster member name")), request_body = ConfirmMember,
    responses((status = 204), (status = 400, body = ErrorBody),
        (status = 401, body = ErrorBody), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody),
        (status = 409, body = ErrorBody), (status = 502, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn remove(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    headers: HeaderMap,
    Path(member): Path<String>,
    body: Result<Json<ConfirmMember>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    allow_platform(&p, "cluster.manage")?;
    member_name(&member)?;
    if confirm(body, &member)? {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "a member is never removed by force here; see docs/runbooks/member-failure.md",
        ));
    }
    let pool = audit_store(&state)?;
    let provider = admin(&state)?;
    let (plan, _) = removal_plan(&state, &member).await?;
    if !plan.blocked.is_empty() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "removal_blocked",
            format!("nothing was changed: {}", plan.blocked.join("; ")),
        ));
    }
    let approval = guard(
        &state,
        &p,
        &headers,
        "cluster.manage",
        &format!("remove:{member}"),
    )
    .await?;
    provider.remove_member(&member).await?;
    super::authz::audit_alone(
        pool,
        &p,
        "cluster.member.remove",
        &member,
        "success",
        None,
        &json!({ "role": p.role, "warnings": plan.warnings, "approval": approval }),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
