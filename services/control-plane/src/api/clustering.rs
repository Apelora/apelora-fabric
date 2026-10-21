// SPDX-License-Identifier: AGPL-3.0-only
//! The Console's Servers area, read-only: members with their database roles, quorum and Incus
//! releases (version skew against the pin), cluster groups, and cluster settings.
//!
//! Members, versions (`GET /1.0?target=<member>`) and groups are read with the restricted
//! deploy identity, which Incus lets see them (REAL INCUS TESTED read, 7.0.1). Incus shows the
//! server configuration only to an unrestricted identity, so the settings use the admin
//! identity (ADR 0003) and return only the keys in [`apelora_domain::clustering::CLUSTER_SETTINGS`].
//! Reads need `cluster.view`. The one change, to the cluster settings, needs
//! `cluster.sensitive_change` and another person's approval, and is audited.

use std::time::Duration;

use apelora_domain::clustering::{
    self, CLUSTER_SETTINGS, ClusterGroupInfo, ClusterMemberDetail, ClusterQuorum, ClusterSetting,
    ClusterVersions,
};
use apelora_domain::plan::DesiredModule;
use apelora_domain::rebalance::{self, RebalanceMove};
use apelora_domain::upgrade::{self, UpgradeView};
use apelora_domain::{ClusterMember, Permission};
use std::collections::BTreeMap;

use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::http::{HeaderMap, StatusCode};
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;

use super::authz::{allow_platform, audit_alone, audit_scoped, begin, commit, guard};
use super::{ApiError, AppState, ErrorBody, Principal, Source, require, source};

/// How long one member's version may take before it is reported as unreadable.
const VERSION_BUDGET: Duration = Duration::from_secs(5);

/// Members with their roles, the database majority and the Incus release each runs.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClusterMembersReport {
    pub source: Source,
    pub clustered: bool,
    /// Empty on a standalone server.
    pub members: Vec<ClusterMemberDetail>,
    pub quorum: ClusterQuorum,
    /// Version skew against the pinned Incus release, and whether an upgrade could start.
    pub versions: ClusterVersions,
    /// Recent BMC power-offs, newest first. Empty when none have been recorded.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub power_offs: Vec<PowerOff>,
    /// Stateless replicas that share a server. The supervisor moves one only when
    /// `APELORA_REBALANCE=1`; this list is the dry run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rebalance: Vec<RebalanceMove>,
    /// The rolling-upgrade assistant. Absent when there is no database.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upgrade: Option<UpgradeView>,
}

/// One recorded power-off of a failed server (ADR 0014). No BMC address.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PowerOff {
    pub at: String,
    pub member: String,
    /// `success`, `failed` or `denied`.
    pub outcome: String,
    pub summary: String,
}

#[derive(Serialize, ToSchema)]
pub struct ClusterGroupList {
    pub source: Source,
    pub items: Vec<ClusterGroupInfo>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClusterSettingsReport {
    pub source: Source,
    /// The Incus identity the values were read with (`admin`: Incus shows its configuration
    /// only to an unrestricted identity).
    pub identity: String,
    pub items: Vec<ClusterSetting>,
}

/// Each member's release, read concurrently, each within [`VERSION_BUDGET`].
async fn member_versions(
    state: &AppState,
    members: &[ClusterMember],
) -> Vec<Result<Option<String>, String>> {
    let mut set = tokio::task::JoinSet::new();
    for (i, m) in members.iter().enumerate() {
        let provider = state.provider.clone();
        let name = m.name.clone();
        set.spawn(async move {
            let r = match tokio::time::timeout(VERSION_BUDGET, provider.member_server_info(&name))
                .await
            {
                Ok(Ok(info)) => Ok(info.server_version),
                Ok(Err(e)) => Err(e.to_string()),
                Err(_) => Err(format!("no answer within {} s", VERSION_BUDGET.as_secs())),
            };
            (i, r)
        });
    }
    let mut out: Vec<Result<Option<String>, String>> =
        vec![Err("the read did not finish".into()); members.len()];
    while let Some(done) = set.join_next().await {
        match done {
            Ok((i, r)) => out[i] = r,
            Err(e) => tracing::error!(error = %e, "servers: a version read panicked"),
        }
    }
    out
}

/// Members with their database role (voter, stand-by, spare), the leader, other roles,
/// failure domain, groups and Incus release; the database majority (how many voters answer,
/// how many are needed, how many more can be lost); and version skew against the pinned
/// Incus release with what blocks an upgrade. Read-only; needs `cluster.view`.
#[utoipa::path(get, path = "/api/v1/cluster/members", tag = "cluster", operation_id = "cluster_members",
    responses((status = 200, body = ClusterMembersReport), (status = 401, body = ErrorBody),
    (status = 403, body = ErrorBody), (status = 502, body = ErrorBody)))]
pub(super) async fn members(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Json<ClusterMembersReport>, ApiError> {
    require(&p, Permission::ReadInventory)?;
    let info = state.provider.server_info().await?;
    let members = if info.clustered {
        state.provider.cluster_members().await?
    } else {
        Vec::new()
    };
    let details: Vec<ClusterMemberDetail> = member_versions(&state, &members)
        .await
        .into_iter()
        .zip(&members)
        .map(|(v, m)| {
            if let Err(e) = &v {
                tracing::debug!(member = %m.name, error = %e, "servers: version not readable");
            }
            clustering::member_detail(m, v)
        })
        .collect();
    let quorum = clustering::quorum(info.clustered, &details);
    let pinned = super::admin::pinned_incus();
    let versions = if info.clustered {
        clustering::versions(&pinned, &details, &quorum)
    } else {
        // A standalone server: compare its own release.
        let one = clustering::member_detail(
            &ClusterMember {
                name: "Incus server".into(),
                url: String::new(),
                roles: Vec::new(),
                status: "Online".into(),
                message: String::new(),
                architecture: info.architectures.first().cloned().unwrap_or_default(),
                failure_domain: String::new(),
                database: false,
                groups: Vec::new(),
                scheduler: "all".into(),
            },
            Ok(info.server_version.clone()),
        );
        clustering::versions(&pinned, &[one], &quorum)
    };
    let (power_offs, rebalance, upgrade) = server_extras(&state, &details).await;
    Ok(Json(ClusterMembersReport {
        source: source(state.provider.mode()),
        clustered: info.clustered,
        members: details,
        quorum,
        versions,
        power_offs,
        rebalance,
        upgrade,
    }))
}

/// Power-off history, the rebalance dry run and the upgrade assistant. All empty when there
/// is no database: the member list itself does not need one.
async fn server_extras(
    state: &AppState,
    details: &[ClusterMemberDetail],
) -> (Vec<PowerOff>, Vec<RebalanceMove>, Option<UpgradeView>) {
    let Ok(pool) = super::cluster_admin::audit_store(state) else {
        return (Vec::new(), Vec::new(), None);
    };
    let power_offs = apelora_persistence::recovery::power_offs(pool, 50)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|e| PowerOff {
            at: super::admin::rfc3339(e.at),
            member: e.member,
            outcome: e.outcome,
            summary: e.summary,
        })
        .collect();
    let rebalance = match (
        apelora_persistence::list_deployments(pool).await,
        state.provider.instances().await,
    ) {
        (Ok(deployed), Ok(instances)) => {
            let mut held = Vec::new();
            for d in &deployed {
                if let Ok(desired) = serde_json::from_value::<Vec<DesiredModule>>(d.desired.clone())
                {
                    held.push((d.application.clone(), d.project.clone(), desired));
                }
            }
            let modules: Vec<rebalance::SpreadModule<'_>> = held
                .iter()
                .flat_map(|(app, project, desired)| {
                    desired.iter().map(move |module| rebalance::SpreadModule {
                        application: app,
                        project,
                        module,
                    })
                })
                .collect();
            let members = state.provider.cluster_members().await.unwrap_or_default();
            rebalance::plan(&members, &instances, &modules)
        }
        _ => Vec::new(),
    };
    let steps = apelora_persistence::upgrades::running(pool)
        .await
        .ok()
        .flatten()
        .map(|r| r.steps());
    let upgrade = Some(upgrade::view(upgrade::order(details), steps.as_deref()));
    (power_offs, rebalance, upgrade)
}

/// Cluster groups and their members (empty on a standalone server). Read-only; needs
/// `cluster.view`.
#[utoipa::path(get, path = "/api/v1/cluster/groups", tag = "cluster", operation_id = "cluster_groups",
    responses((status = 200, body = ClusterGroupList), (status = 401, body = ErrorBody),
    (status = 403, body = ErrorBody), (status = 502, body = ErrorBody)))]
pub(super) async fn groups(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Json<ClusterGroupList>, ApiError> {
    require(&p, Permission::ReadInventory)?;
    let items = state.provider.cluster_groups().await?;
    Ok(Json(ClusterGroupList {
        source: source(state.provider.mode()),
        items,
    }))
}

/// Cluster-wide settings (offline and healing thresholds, database voters and stand-bys,
/// join-token lifetime, image replication and updates): the value set, Incus's default and
/// whether the default applies. Read with the admin identity, since Incus shows its
/// configuration only to an unrestricted one (503 `cluster_admin_not_configured` without it);
/// only these keys are returned. Read-only; needs `cluster.view`.
#[utoipa::path(get, path = "/api/v1/cluster/settings", tag = "cluster", operation_id = "cluster_settings",
    responses((status = 200, body = ClusterSettingsReport), (status = 401, body = ErrorBody),
    (status = 403, body = ErrorBody), (status = 502, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn settings(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Json<ClusterSettingsReport>, ApiError> {
    require(&p, Permission::ReadInventory)?;
    let admin = super::cluster_admin::admin(&state)?;
    let keys: Vec<&str> = CLUSTER_SETTINGS.iter().map(|(k, _, _)| *k).collect();
    let values = admin.server_config(&keys).await?;
    // Defaults are documentation: without them the values still stand.
    let defaults = state.provider.config_defaults().await.unwrap_or_else(|e| {
        tracing::debug!(error = %e, "servers: configuration defaults not readable");
        Default::default()
    });
    Ok(Json(ClusterSettingsReport {
        source: source(admin.mode()),
        identity: "admin".into(),
        items: clustering::settings(&values, &defaults),
    }))
}

/// A change to cluster settings: each key is one of the listed settings; an empty or null value
/// removes it so Incus's default applies again.
#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChangeSettings {
    pub changes: BTreeMap<String, Option<String>>,
    /// Only report what would change and what to watch for; nothing is changed and no approval
    /// is needed.
    #[serde(default)]
    pub dry_run: bool,
}

/// One setting in a change: what it is now, what it becomes, and what to know first.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SettingChange {
    pub key: String,
    pub label: String,
    /// The value set now; absent when Incus's default applies.
    pub from: Option<String>,
    /// The value after the change; absent when the setting is removed.
    pub to: Option<String>,
    pub warnings: Vec<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SettingsChangeReport {
    /// False for a dry run.
    pub applied: bool,
    pub changes: Vec<SettingChange>,
    /// The approval target to ask for (`POST /approvals`, action `cluster.sensitive_change`).
    pub approval_target: String,
}

/// The approval target for a set of settings: the same keys always give the same target.
fn approval_target(keys: &[&str]) -> String {
    format!("cluster-settings:{}", keys.join("/"))
}

const MAX_CHANGED_AT_ONCE: usize = 9;

/// Changes cluster settings through Incus with the admin identity. Every key must be one the
/// Servers page lists and every value is checked. Settings that already hold the value are left
/// out; nothing to change is refused. With `dryRun` it only reports (what each becomes and what
/// to watch for). Applying needs `cluster.sensitive_change` and another person's approval of
/// `cluster-settings:<keys joined by />` (`Apelora-Approval`), spent before the change, and is
/// audited with the old and new values.
#[utoipa::path(patch, path = "/api/v1/cluster/settings", tag = "cluster", operation_id = "change_cluster_settings",
    request_body = ChangeSettings,
    responses((status = 200, body = SettingsChangeReport), (status = 400, body = ErrorBody),
    (status = 401, body = ErrorBody), (status = 403, body = ErrorBody), (status = 409, body = ErrorBody),
    (status = 502, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn change_settings(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    headers: HeaderMap,
    body: Result<Json<ChangeSettings>, JsonRejection>,
) -> Result<Json<SettingsChangeReport>, ApiError> {
    allow_platform(&p, "cluster.sensitive_change")?;
    let Json(req) =
        body.map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, "invalid_request", e.body_text()))?;
    let bad = |m: String| ApiError::new(StatusCode::BAD_REQUEST, "invalid_setting", m);
    if req.changes.is_empty() || req.changes.len() > MAX_CHANGED_AT_ONCE {
        return Err(bad(format!(
            "change one to {MAX_CHANGED_AT_ONCE} settings at a time"
        )));
    }
    for (key, value) in &req.changes {
        clustering::check_setting(key, value.as_deref().unwrap_or("")).map_err(bad)?;
    }
    let pool = super::cluster_admin::audit_store(&state)?;
    let admin = super::cluster_admin::admin(&state)?;
    let keys: Vec<&str> = CLUSTER_SETTINGS.iter().map(|(k, _, _)| *k).collect();
    let current = admin.server_config(&keys).await?;

    let mut changes = Vec::new();
    let mut actual = BTreeMap::new();
    for (key, label, _) in CLUSTER_SETTINGS {
        let Some(wanted) = req.changes.get(*key) else {
            continue;
        };
        let to = wanted.clone().filter(|v| !v.is_empty());
        let from = current.get(*key).filter(|v| !v.is_empty()).cloned();
        if from == to {
            continue;
        }
        changes.push(SettingChange {
            key: (*key).to_owned(),
            label: (*label).to_owned(),
            warnings: clustering::setting_warnings(key, to.as_deref().unwrap_or("")),
            from,
            to: to.clone(),
        });
        actual.insert((*key).to_owned(), to);
    }
    if changes.is_empty() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "nothing_to_change",
            "every setting already has that value; nothing was changed",
        ));
    }
    let target = approval_target(&changes.iter().map(|c| c.key.as_str()).collect::<Vec<_>>());
    if req.dry_run {
        return Ok(Json(SettingsChangeReport {
            applied: false,
            changes,
            approval_target: target,
        }));
    }
    let approval = guard(&state, &p, &headers, "cluster.sensitive_change", &target).await?;
    admin.set_server_config(&actual).await?;
    audit_alone(
        pool,
        &p,
        "cluster.settings.change",
        &target,
        "success",
        None,
        &json!({
            "changes": changes.iter().map(|c| json!({ "key": c.key, "from": c.from, "to": c.to })).collect::<Vec<_>>(),
            "approval": approval,
        }),
    )
    .await?;
    Ok(Json(SettingsChangeReport {
        applied: true,
        changes,
        approval_target: target,
    }))
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeCommand {
    /// `start`, `evacuate`, `upgraded`, `restore` or `abandon`.
    pub action: String,
    /// The member, for evacuate, upgraded and restore.
    #[serde(default)]
    pub member: String,
    /// `upgrade` to start or abandon; the member's name for the other actions.
    pub confirm: String,
}

/// One step of the rolling upgrade. Apelora evacuates and restores. It does not install Incus:
/// `upgraded` records that the package was upgraded on the server. Needs `cluster.evacuate`.
#[utoipa::path(post, path = "/api/v1/cluster/upgrade", tag = "cluster", operation_id = "cluster_upgrade",
    request_body = UpgradeCommand,
    responses((status = 200, body = UpgradeView), (status = 202, body = UpgradeView),
        (status = 400, body = ErrorBody), (status = 401, body = ErrorBody), (status = 403, body = ErrorBody),
        (status = 409, body = ErrorBody), (status = 502, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn upgrade(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    body: Result<Json<UpgradeCommand>, JsonRejection>,
) -> Result<(StatusCode, Json<UpgradeView>), ApiError> {
    allow_platform(&p, "cluster.evacuate")?;
    let Json(cmd) =
        body.map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, "invalid_request", e.body_text()))?;
    let pool = super::cluster_admin::audit_store(&state)?;
    let info = state.provider.server_info().await?;
    let members = if info.clustered {
        state.provider.cluster_members().await?
    } else {
        Vec::new()
    };
    let details: Vec<ClusterMemberDetail> = member_versions(&state, &members)
        .await
        .into_iter()
        .zip(&members)
        .map(|(v, m)| clustering::member_detail(m, v))
        .collect();
    let order = upgrade::order(&details);
    match cmd.action.as_str() {
        "start" => {
            if cmd.confirm != "upgrade" {
                return Err(ApiError::new(
                    StatusCode::BAD_REQUEST,
                    "confirmation_mismatch",
                    "type upgrade to start",
                ));
            }
            if apelora_persistence::upgrades::running(pool)
                .await?
                .is_some()
            {
                return Err(ApiError::new(
                    StatusCode::CONFLICT,
                    "upgrade_in_progress",
                    "an upgrade is already in progress",
                ));
            }
            let quorum = clustering::quorum(info.clustered, &details);
            let versions = clustering::versions(&super::admin::pinned_incus(), &details, &quorum);
            if !versions.ready {
                return Err(ApiError::new(
                    StatusCode::CONFLICT,
                    "upgrade_blocked",
                    versions.blockers.join("; "),
                ));
            }
            if order.is_empty() {
                return Err(ApiError::new(
                    StatusCode::CONFLICT,
                    "upgrade_blocked",
                    "the cluster reports no members",
                ));
            }
            let steps: Vec<apelora_domain::upgrade::UpgradeStep> = order
                .iter()
                .map(|m| apelora_domain::upgrade::UpgradeStep {
                    member: m.clone(),
                    phase: "pending".into(),
                })
                .collect();
            let mut tx = begin(pool).await?;
            apelora_persistence::upgrades::start(&mut tx, &p.actor_ref(), &steps).await?;
            audit_scoped(
                &mut tx,
                &p,
                "cluster.upgrade.start",
                "cluster",
                "success",
                None,
                &json!({ "order": order, "role": p.role }),
            )
            .await?;
            commit(tx).await?;
            Ok((StatusCode::OK, Json(upgrade::view(order, Some(&steps)))))
        }
        "abandon" => {
            if cmd.confirm != "upgrade" {
                return Err(ApiError::new(
                    StatusCode::BAD_REQUEST,
                    "confirmation_mismatch",
                    "type upgrade to abandon the run",
                ));
            }
            let Some(run) = apelora_persistence::upgrades::running(pool).await? else {
                return Err(ApiError::new(
                    StatusCode::CONFLICT,
                    "no_upgrade",
                    "no upgrade is in progress",
                ));
            };
            let mut tx = begin(pool).await?;
            apelora_persistence::upgrades::abandon(&mut tx, run.id).await?;
            audit_scoped(
                &mut tx,
                &p,
                "cluster.upgrade.abandon",
                "cluster",
                "success",
                None,
                &json!({ "role": p.role }),
            )
            .await?;
            commit(tx).await?;
            Ok((StatusCode::OK, Json(upgrade::view(order, None))))
        }
        "evacuate" | "upgraded" | "restore" => {
            if cmd.confirm != cmd.member || cmd.member.is_empty() {
                return Err(ApiError::new(
                    StatusCode::BAD_REQUEST,
                    "confirmation_mismatch",
                    "type the member's name to take this step",
                ));
            }
            let Some(run) = apelora_persistence::upgrades::running(pool).await? else {
                return Err(ApiError::new(
                    StatusCode::CONFLICT,
                    "no_upgrade",
                    "no upgrade is in progress",
                ));
            };
            let mut steps = run.steps();
            let current = upgrade::view(order.clone(), Some(&steps));
            if current.next != cmd.action || current.member.as_deref() != Some(cmd.member.as_str())
            {
                return Err(ApiError::new(
                    StatusCode::CONFLICT,
                    "upgrade_step",
                    current.summary,
                ));
            }
            if cmd.action == "evacuate" {
                let (plan, _) = super::cluster_admin::evacuation_plan(&state, &cmd.member).await?;
                if !plan.blocked.is_empty() {
                    return Err(ApiError::new(
                        StatusCode::CONFLICT,
                        "evacuation_blocked",
                        format!("nothing was changed: {}", plan.blocked.join("; ")),
                    ));
                }
                let provider = super::cluster_admin::admin(&state)?;
                provider.member_state(&cmd.member, false).await?;
            }
            if cmd.action == "restore" {
                let provider = super::cluster_admin::admin(&state)?;
                provider.member_state(&cmd.member, true).await?;
            }
            let phase = upgrade::advance(
                steps
                    .iter()
                    .find(|s| s.member == cmd.member)
                    .map(|s| s.phase.as_str())
                    .unwrap_or(""),
                &cmd.action,
            )
            .ok_or_else(|| {
                ApiError::new(
                    StatusCode::CONFLICT,
                    "upgrade_step",
                    "that step is not the next one",
                )
            })?;
            if let Some(step) = steps.iter_mut().find(|s| s.member == cmd.member) {
                step.phase = phase.into();
            }
            let done = steps.iter().all(|s| s.phase == "restored");
            let mut tx = begin(pool).await?;
            apelora_persistence::upgrades::save_steps(&mut tx, run.id, &steps, done).await?;
            audit_scoped(
                &mut tx,
                &p,
                &format!("cluster.upgrade.{}", cmd.action),
                &cmd.member,
                "success",
                None,
                &json!({ "phase": phase, "role": p.role }),
            )
            .await?;
            commit(tx).await?;
            let status = if cmd.action == "upgraded" {
                StatusCode::OK
            } else {
                StatusCode::ACCEPTED
            };
            Ok((status, Json(upgrade::view(order, Some(&steps)))))
        }
        _ => Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "action is start, evacuate, upgraded, restore or abandon",
        )),
    }
}
