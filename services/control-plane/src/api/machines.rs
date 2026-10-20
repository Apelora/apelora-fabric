// SPDX-License-Identifier: AGPL-3.0-only
//! Machines and their enrolment as cluster members (Phase 09, ADR 0002,
//! `docs/design/node-enrolment.md`). The record says who registered, approved and admitted a
//! machine; Incus says whether it is a member and whether it is held from placement. Join
//! tokens are returned once and never stored, logged or audited (ADR 0003).

use apelora_domain::enrolment::{
    self, AdmissionInput, Capabilities, MachineState, Observation, ProvisionerKind,
};
use apelora_domain::preflight::Check;
use apelora_domain::{ClusterMember, PendingJoin, Permission};
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;

use super::admin::rfc3339;
use super::cluster_admin::{admin, audit_store, lifetime_warnings, member_name};
use super::{ApiError, AppState, ErrorBody, Principal, Source, require, source};

/// Where the enrolment script lives in the source tree and release archives.
const ENROL_SCRIPT: &str = "scripts/enrol/enrol-member.sh";

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Machine {
    pub name: String,
    pub provider: ProvisionerKind,
    /// What the provider can do without someone at the server.
    pub capabilities: Capabilities,
    pub description: String,
    pub state: MachineState,
    /// The cluster member's status (`Online`, `Offline`, `Evacuated`…) once it has joined.
    pub member_status: Option<String>,
    pub architecture: Option<String>,
    /// What the administrator does next.
    pub next_step: String,
    pub warnings: Vec<String>,
    pub registered_by: String,
    pub registered_at: String,
    pub approved_by: Option<String>,
    pub approved_at: Option<String>,
    /// When the approval's join token stops working, as Incus reported it.
    pub approval_expires_at: Option<String>,
    pub admitted_by: Option<String>,
    pub admitted_at: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct MachineList {
    pub source: Source,
    pub items: Vec<Machine>,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RegisterMachine {
    /// The name it will have as a cluster member.
    pub name: String,
    /// Defaults to `manual`.
    #[serde(default)]
    pub provider: Option<ProvisionerKind>,
    #[serde(default)]
    pub description: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MachineApproved {
    pub name: String,
    /// RFC 3339, as Incus reports it.
    pub expires_at: String,
    /// Single-use join token for the enrolment script. Shown only in this response.
    pub token: String,
    /// What to run on the server; it asks for the token without echoing it.
    pub enrol_command: String,
    pub warnings: Vec<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Admission {
    pub source: Source,
    pub name: String,
    /// No check failed: the machine can be admitted.
    pub admissible: bool,
    pub checks: Vec<Check>,
}

fn machine_view(
    r: apelora_persistence::MachineRecord,
    members: &[ClusterMember],
    pending: &[PendingJoin],
) -> Machine {
    let provider = ProvisionerKind::parse(&r.provider).unwrap_or(ProvisionerKind::Manual);
    let member = members.iter().find(|m| m.name == r.name);
    let o = Observation {
        approved: r.approved_at.is_some(),
        admitted: r.admitted_at.is_some(),
        member,
        token_pending: pending.iter().any(|p| p.member_name == r.name),
    };
    let state = enrolment::machine_state(&o);
    Machine {
        capabilities: provider.capabilities(),
        provider,
        state,
        member_status: member.map(|m| m.status.clone()),
        architecture: member.map(|m| m.architecture.clone()),
        next_step: enrolment::next_step(state, provider).into(),
        warnings: enrolment::warnings(&o),
        name: r.name,
        description: r.description,
        registered_by: r.registered_by,
        registered_at: rfc3339(r.registered_at),
        approved_by: r.approved_by,
        approved_at: r.approved_at.map(rfc3339),
        approval_expires_at: r.approval_expires_at,
        admitted_by: r.admitted_by,
        admitted_at: r.admitted_at.map(rfc3339),
    }
}

fn conflict(code: &'static str, message: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::CONFLICT, code, message)
}

/// Machines being enrolled, with their state derived from Incus. Needs `manage-infrastructure`.
#[utoipa::path(get, path = "/api/v1/machines", tag = "cluster", operation_id = "list_machines",
    responses((status = 200, body = MachineList), (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody), (status = 502, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn list(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Json<MachineList>, ApiError> {
    require(&p, Permission::ManageInfrastructure)?;
    let pool = audit_store(&state)?;
    let provider = admin(&state)?;
    let records = apelora_persistence::machines(pool).await?;
    let members = provider.cluster_members().await?;
    let pending = provider.join_tokens().await?;
    Ok(Json(MachineList {
        source: source(provider.mode()),
        items: records
            .into_iter()
            .map(|r| machine_view(r, &members, &pending))
            .collect(),
    }))
}

/// Registers a machine. A name that is already an eligible member is adopted as admitted.
/// Needs `manage-infrastructure`; audited.
#[utoipa::path(post, path = "/api/v1/machines", tag = "cluster", operation_id = "register_machine", request_body = RegisterMachine,
    responses((status = 201, body = Machine), (status = 400, body = ErrorBody), (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody), (status = 409, body = ErrorBody), (status = 502, body = ErrorBody),
        (status = 503, body = ErrorBody)))]
pub(super) async fn register(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    body: Result<Json<RegisterMachine>, JsonRejection>,
) -> Result<(StatusCode, Json<Machine>), ApiError> {
    require(&p, Permission::ManageInfrastructure)?;
    let Json(req) =
        body.map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, "invalid_request", e.body_text()))?;
    member_name(&req.name)?;
    if req.description.chars().count() > 200 {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "the description is longer than 200 characters",
        ));
    }
    let kind = req.provider.unwrap_or(ProvisionerKind::Manual);
    if kind != ProvisionerKind::Manual {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "provisioner_not_configured",
            format!(
                "no {} provisioner is configured; register the machine as manual",
                kind.as_str()
            ),
        ));
    }
    let pool = audit_store(&state)?;
    let provider = admin(&state)?;
    let members = provider.cluster_members().await?;
    let adopted = members
        .iter()
        .any(|m| m.name == req.name && m.scheduler != enrolment::HELD);
    let record = apelora_persistence::register_machine(
        pool,
        &req.name,
        kind.as_str(),
        req.description.trim(),
        &p.name,
        adopted,
    )
    .await?;
    apelora_persistence::audit(
        pool,
        &p.name,
        "machine.register",
        &req.name,
        &json!({ "role": p.role, "provider": kind, "adopted": adopted }),
    )
    .await?;
    let pending = provider.join_tokens().await?;
    Ok((
        StatusCode::CREATED,
        Json(machine_view(record, &members, &pending)),
    ))
}

/// Approves a machine: asks Incus for its single-use join token, returned only in this
/// response with the command to run on the server. Needs `manage-infrastructure`; audited
/// without the token.
#[utoipa::path(post, path = "/api/v1/machines/{name}/approve", tag = "cluster", operation_id = "approve_machine",
    params(("name" = String, Path, description = "Machine name")),
    responses((status = 201, body = MachineApproved), (status = 400, body = ErrorBody),
        (status = 401, body = ErrorBody), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody),
        (status = 409, body = ErrorBody), (status = 502, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn approve(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(name): Path<String>,
) -> Result<Response, ApiError> {
    require(&p, Permission::ManageInfrastructure)?;
    member_name(&name)?;
    let pool = audit_store(&state)?;
    let provider = admin(&state)?;
    apelora_persistence::machine(pool, &name).await?;
    if provider
        .cluster_members()
        .await?
        .iter()
        .any(|m| m.name == name)
    {
        return Err(conflict(
            "already_a_member",
            format!("{name} is already a cluster member"),
        ));
    }
    if provider
        .join_tokens()
        .await?
        .iter()
        .any(|t| t.member_name == name)
    {
        return Err(conflict(
            "approval_pending",
            format!("{name} already has an unused join token; withdraw the approval first"),
        ));
    }
    let t = provider.create_join_token(&name).await?;
    apelora_persistence::approve_machine(pool, &name, &p.name, &t.expires_at).await?;
    apelora_persistence::audit(
        pool,
        &p.name,
        "machine.approve",
        &name,
        &json!({ "role": p.role, "expiresAt": t.expires_at }),
    )
    .await?;
    let warnings = lifetime_warnings(&t.expires_at, time::OffsetDateTime::now_utc());
    let mut res = (
        StatusCode::CREATED,
        Json(MachineApproved {
            enrol_command: format!(
                "sudo bash {ENROL_SCRIPT} --name {name} --address <this server's cluster address>"
            ),
            name,
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

/// Withdraws an approval: revokes the machine's unused join token. Needs
/// `manage-infrastructure`; audited.
#[utoipa::path(delete, path = "/api/v1/machines/{name}/approval", tag = "cluster", operation_id = "withdraw_machine_approval",
    params(("name" = String, Path, description = "Machine name")),
    responses((status = 204), (status = 400, body = ErrorBody), (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 502, body = ErrorBody),
        (status = 503, body = ErrorBody)))]
pub(super) async fn withdraw(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    require(&p, Permission::ManageInfrastructure)?;
    member_name(&name)?;
    let pool = audit_store(&state)?;
    let provider = admin(&state)?;
    apelora_persistence::machine(pool, &name).await?;
    let revoked = provider.revoke_join_token(&name).await?;
    apelora_persistence::withdraw_approval(pool, &name).await?;
    apelora_persistence::audit(
        pool,
        &p.name,
        "machine.approval.withdraw",
        &name,
        &json!({ "role": p.role, "tokenRevoked": revoked }),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The member and the facts admission looks at; 409 when the machine has not joined.
async fn admission(state: &AppState, name: &str) -> Result<Admission, ApiError> {
    let provider = admin(state)?;
    let members = provider.cluster_members().await?;
    let member = members.iter().find(|m| m.name == name).ok_or_else(|| {
        conflict(
            "not_a_member",
            format!("{name} has not joined the cluster yet"),
        )
    })?;
    let server = provider.member_server_info(name).await?;
    let cluster = provider.server_info().await?;
    let pools = provider.storage_pools().await?;
    let networks = provider.networks().await?;
    let checks = enrolment::admission_checks(&AdmissionInput {
        member,
        server: &server,
        cluster_version: cluster.server_version.as_deref(),
        others: &members,
        pools: &pools,
        networks: &networks,
    });
    Ok(Admission {
        source: source(provider.mode()),
        name: name.to_owned(),
        admissible: enrolment::admissible(&checks),
        checks,
    })
}

/// The admission checks for a joined machine (dry run; changes nothing). Needs
/// `manage-infrastructure`.
#[utoipa::path(get, path = "/api/v1/machines/{name}/admission", tag = "cluster", operation_id = "machine_admission",
    params(("name" = String, Path, description = "Machine name")),
    responses((status = 200, body = Admission), (status = 400, body = ErrorBody), (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody),
        (status = 502, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn preview_admission(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(name): Path<String>,
) -> Result<Json<Admission>, ApiError> {
    require(&p, Permission::ManageInfrastructure)?;
    member_name(&name)?;
    let pool = audit_store(&state)?;
    apelora_persistence::machine(pool, &name).await?;
    Ok(Json(admission(&state, &name).await?))
}

/// Admits a joined machine: re-runs the admission checks and, when none fails, lets Incus
/// place instances on it. Needs `manage-infrastructure`; audited.
#[utoipa::path(post, path = "/api/v1/machines/{name}/admit", tag = "cluster", operation_id = "admit_machine",
    params(("name" = String, Path, description = "Machine name")),
    responses((status = 200, body = Admission), (status = 400, body = ErrorBody), (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody),
        (status = 502, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn admit(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(name): Path<String>,
) -> Result<Json<Admission>, ApiError> {
    require(&p, Permission::ManageInfrastructure)?;
    member_name(&name)?;
    let pool = audit_store(&state)?;
    apelora_persistence::machine(pool, &name).await?;
    let result = admission(&state, &name).await?;
    if !result.admissible {
        let failed: Vec<String> = result
            .checks
            .iter()
            .filter(|c| c.status == apelora_domain::preflight::CheckStatus::Fail)
            .map(|c| format!("{}: {}", c.title, c.detail))
            .collect();
        return Err(conflict(
            "admission_blocked",
            format!("nothing was changed: {}", failed.join("; ")),
        ));
    }
    admin(&state)?.set_member_held(&name, false).await?;
    apelora_persistence::admit_machine(pool, &name, &p.name).await?;
    let summary: Vec<String> = result
        .checks
        .iter()
        .map(|c| format!("{}={:?}", c.id, c.status))
        .collect();
    apelora_persistence::audit(
        pool,
        &p.name,
        "machine.admit",
        &name,
        &json!({ "role": p.role, "checks": summary }),
    )
    .await?;
    Ok(Json(result))
}

/// Forgets a machine that is not a cluster member (revoking an unused join token). Needs
/// `manage-infrastructure`; audited.
#[utoipa::path(delete, path = "/api/v1/machines/{name}", tag = "cluster", operation_id = "remove_machine",
    params(("name" = String, Path, description = "Machine name")),
    responses((status = 204), (status = 400, body = ErrorBody), (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody),
        (status = 502, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn remove(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    require(&p, Permission::ManageInfrastructure)?;
    member_name(&name)?;
    let pool = audit_store(&state)?;
    let provider = admin(&state)?;
    apelora_persistence::machine(pool, &name).await?;
    if provider
        .cluster_members()
        .await?
        .iter()
        .any(|m| m.name == name)
    {
        return Err(conflict(
            "still_a_member",
            format!("{name} is a cluster member; evacuate it and remove it from the cluster first"),
        ));
    }
    let revoked = provider.revoke_join_token(&name).await?;
    apelora_persistence::remove_machine(pool, &name).await?;
    apelora_persistence::audit(
        pool,
        &p.name,
        "machine.remove",
        &name,
        &json!({ "role": p.role, "tokenRevoked": revoked }),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
