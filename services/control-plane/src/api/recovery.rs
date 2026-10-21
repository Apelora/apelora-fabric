// SPDX-License-Identifier: AGPL-3.0-only
//! Failed members (HA slice B, ADR 0013): their fencing state as the worker last recorded it,
//! and per instance whether it restarts on another member (and where) or waits (and why). A
//! person may confirm that a failed server is powered off, which fences it.
//!
//! The worker's supervisor leader decides and acts; this module reads its records
//! (`member_failures`) and Incus, and computes the same plan with
//! [`apelora_domain::recovery::plan_recovery`].

use apelora_domain::evacuation::Deployed;
use apelora_domain::fencing::{FenceCondition, FenceEvidence, FenceView};
use apelora_domain::plan::DesiredModule;
use apelora_domain::recovery::{RecoveryAction, RecoveryItem, plan_recovery};
use apelora_domain::{Permission, ProviderMode};
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;

use super::admin::rfc3339;
use super::authz::{allow_platform, guard};
use super::cluster_admin::{audit_store, member_name};
use super::{ApiError, AppState, Auth, ErrorBody, Principal, Source, require, source};

/// One failed member: its fencing and what happens to each of its instances.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryMember {
    pub member: String,
    /// Incus's status (`Offline` while it is failed).
    pub status: String,
    /// The failure record; `None` until the supervisor has seen it.
    pub failure: Option<String>,
    /// When Apelora noticed (RFC 3339).
    pub detected_at: Option<String>,
    /// Incus's last heartbeat from it (RFC 3339), when its message gives one.
    pub last_heartbeat: Option<String>,
    pub fencing: FenceView,
    pub fenced_at: Option<String>,
    pub evidence: Option<FenceEvidence>,
    /// Who confirmed the server is powered off, when someone did.
    pub confirmed_by: Option<String>,
    /// When the recovery decision was recorded (once per failure).
    pub decided_at: Option<String>,
    pub items: Vec<RecoveryItem>,
}

#[derive(Serialize, ToSchema)]
pub struct RecoveryReport {
    pub source: Source,
    /// Failed members, by name.
    pub items: Vec<RecoveryMember>,
}

/// A person's confirmation that a failed server is powered off.
#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FenceConfirmation {
    /// The member's name again, typed by the person.
    pub confirm: String,
    /// Must be `true`: the server is switched off (or cut off from the shared storage).
    pub powered_off: bool,
}

/// The fencing state the worker has not recorded yet.
fn not_seen() -> FenceView {
    FenceView {
        condition: FenceCondition::Waiting,
        reason: "the supervisor has not looked at this failure yet".into(),
        next_step: Some(
            "Wait for the next supervision pass (the worker runs it every APELORA_SUPERVISE_SECS)"
                .into(),
        ),
        evidence: None,
        lease_state: None,
        observed_by: None,
        observed_at: None,
    }
}

/// Every failed member with its plan, read from the worker's records and Incus.
pub(super) async fn report(
    state: &AppState,
) -> Result<(Vec<RecoveryMember>, ProviderMode), ApiError> {
    let provider = &state.provider;
    let members = provider.cluster_members().await?;
    let offline: Vec<_> = members.iter().filter(|m| m.status == "Offline").collect();
    if offline.is_empty() {
        return Ok((Vec::new(), provider.mode()));
    }
    let (failures, records) = match &state.auth {
        Auth::Tokens(pool) => (
            apelora_persistence::recovery::open_failures(pool).await?,
            apelora_persistence::list_deployments(pool).await?,
        ),
        Auth::DisabledForDevelopment => (Vec::new(), Vec::new()),
    };
    let instances = provider.instances().await?;
    let pools = provider.storage_pools().await?;
    let parsed: Vec<(String, String, Vec<DesiredModule>)> = records
        .into_iter()
        .filter_map(|r| {
            serde_json::from_value(r.desired)
                .ok()
                .map(|d| (r.application, r.project, d))
        })
        .collect();
    let deployed: Vec<Deployed<'_>> = parsed
        .iter()
        .map(|(application, project, desired)| Deployed {
            application,
            project,
            desired,
        })
        .collect();
    let mut out = Vec::new();
    for m in offline {
        let row = failures.iter().find(|f| f.member == m.name);
        let fenced = row.is_some_and(|f| f.fenced_at.is_some());
        let mut items = plan_recovery(m, fenced, &members, &instances, &deployed, &pools);
        // Instances already restarted elsewhere for this failure are no longer on the member:
        // list them from the decision with where they run now.
        if let Some(decided) = row
            .and_then(|f| f.decision.clone())
            .and_then(|d| serde_json::from_value::<Vec<RecoveryItem>>(d).ok())
        {
            for d in decided
                .into_iter()
                .filter(|d| d.action == RecoveryAction::Restart)
            {
                if items
                    .iter()
                    .any(|i| i.project == d.project && i.instance == d.instance)
                {
                    continue;
                }
                let now = instances
                    .iter()
                    .find(|i| i.project == d.project && i.name == d.instance);
                items.push(RecoveryItem {
                    target: now.map(|i| i.location.clone()),
                    reason: match now {
                        Some(i) => format!("restarted on {}: {}", i.location, i.status),
                        None => "no longer exists".into(),
                    },
                    ..d
                });
            }
        }
        let fencing = row
            .and_then(|f| f.fencing.clone())
            .and_then(|v| serde_json::from_value::<FenceView>(v).ok())
            .unwrap_or_else(not_seen);
        out.push(RecoveryMember {
            member: m.name.clone(),
            status: m.status.clone(),
            failure: row.map(|f| f.id.to_string()),
            detected_at: row.map(|f| rfc3339(f.detected_at)),
            last_heartbeat: row.and_then(|f| f.last_heartbeat).map(rfc3339),
            fenced_at: row.and_then(|f| f.fenced_at).map(rfc3339),
            evidence: row
                .and_then(|f| f.fence_evidence.clone())
                .and_then(|v| serde_json::from_value(v).ok()),
            decided_at: row.and_then(|f| f.decided_at).map(rfc3339),
            confirmed_by: row.and_then(|f| f.confirmed_by.clone()),
            fencing,
            items,
        });
    }
    Ok((out, provider.mode()))
}

/// Failed members: whether each is fenced (and the evidence), or why Apelora waits; and per
/// instance whether it restarts on another server (and where) or waits (and why). Read-only;
/// needs `cluster.view`.
#[utoipa::path(get, path = "/api/v1/cluster/recovery", tag = "cluster", operation_id = "cluster_recovery",
    responses((status = 200, body = RecoveryReport), (status = 401, body = ErrorBody),
    (status = 403, body = ErrorBody), (status = 502, body = ErrorBody)))]
pub(super) async fn recovery(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Json<RecoveryReport>, ApiError> {
    require(&p, Permission::ReadInventory)?;
    let (items, mode) = report(&state).await?;
    Ok(Json(RecoveryReport {
        source: source(mode),
        items,
    }))
}

/// Confirms that a failed server is powered off (or cut off from the shared storage). This
/// fences it: Apelora then restarts its modules on shared storage on other servers. Refused
/// unless Incus reports the member Offline. Needs `cluster.fence_confirm` and another person's
/// approval; the member's name typed again; audited.
#[utoipa::path(post, path = "/api/v1/cluster/members/{member}/fencing", tag = "cluster", operation_id = "confirm_fenced",
    params(("member" = String, Path, description = "Cluster member name")), request_body = FenceConfirmation,
    responses((status = 200, body = RecoveryMember), (status = 400, body = ErrorBody),
        (status = 401, body = ErrorBody), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody),
        (status = 409, body = ErrorBody), (status = 502, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn confirm(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    headers: HeaderMap,
    Path(member): Path<String>,
    body: Result<Json<FenceConfirmation>, JsonRejection>,
) -> Result<Json<RecoveryMember>, ApiError> {
    allow_platform(&p, "cluster.fence_confirm")?;
    member_name(&member)?;
    let Json(req) =
        body.map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, "invalid_request", e.body_text()))?;
    if req.confirm != member {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "confirmation_mismatch",
            format!("type the member's name ({member}) to confirm"),
        ));
    }
    if !req.powered_off {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "confirm only a server that is switched off (poweredOff: true)",
        ));
    }
    let pool = audit_store(&state)?;
    let found = state
        .provider
        .cluster_members()
        .await?
        .into_iter()
        .find(|m| m.name == member)
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "not_found",
                format!("{member} is not a cluster member"),
            )
        })?;
    if found.status != "Offline" {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "member_not_offline",
            format!(
                "{member} is {}, not Offline: only a failed server can be confirmed powered off",
                found.status
            ),
        ));
    }
    let approval = guard(
        &state,
        &p,
        &headers,
        "cluster.fence_confirm",
        &format!("fence:{member}"),
    )
    .await?;
    let mut tx = super::authz::begin(pool).await?;
    let row = apelora_persistence::recovery::confirm_powered_off(&mut tx, &member, &p.actor_ref())
        .await?;
    super::authz::audit_as(
        &mut tx,
        &p,
        "cluster.member.fence.confirmed",
        &member,
        &json!({ "failure": row.id, "approval": approval, "message": found.message }),
    )
    .await?;
    super::authz::commit(tx).await?;
    tracing::warn!(member = %member, actor = %p.actor_ref(), failure = %row.id, "recovery: a person confirmed the failed server is powered off");
    let (items, _) = report(&state).await?;
    let one = items
        .into_iter()
        .find(|m| m.member == member)
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::CONFLICT,
                "member_not_offline",
                format!("{member} is no longer offline"),
            )
        })?;
    Ok(Json(one))
}
