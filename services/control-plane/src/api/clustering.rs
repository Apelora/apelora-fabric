// SPDX-License-Identifier: AGPL-3.0-only
//! The Console's Servers area, read-only: members with their database roles, quorum and Incus
//! releases (version skew against the pin), cluster groups, and cluster settings.
//!
//! Members, versions (`GET /1.0?target=<member>`) and groups are read with the restricted
//! deploy identity, which Incus lets see them (REAL INCUS TESTED read, 7.0.1). Incus shows the
//! server configuration only to an unrestricted identity, so the settings use the admin
//! identity (ADR 0003) and return only the keys in [`apelora_domain::clustering::CLUSTER_SETTINGS`].
//! Nothing here changes Incus; every route needs `cluster.view`.

use std::time::Duration;

use apelora_domain::clustering::{
    self, CLUSTER_SETTINGS, ClusterGroupInfo, ClusterMemberDetail, ClusterQuorum, ClusterSetting,
    ClusterVersions,
};
use apelora_domain::{ClusterMember, Permission};
use axum::extract::State;
use axum::{Extension, Json};
use serde::Serialize;
use utoipa::ToSchema;

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
    Ok(Json(ClusterMembersReport {
        source: source(state.provider.mode()),
        clustered: info.clustered,
        members: details,
        quorum,
        versions,
    }))
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
