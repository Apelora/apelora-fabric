// SPDX-License-Identifier: AGPL-3.0-only
//! Application management: the list and overview of applications with their derived status,
//! creating and describing applications, server-side drafts, published versions, and the
//! start / stop / restart operations.
//!
//! **Nothing here trusts the browser for state.** Design status comes from the stored draft and
//! versions; runtime status is derived from the recorded deployment, what Incus reports, the
//! supervisor's health evidence and the latest operation (`apelora_domain::lifecycle`). If Incus
//! cannot be read the answer is `unknown` and marked stale. Every handler authorises against the
//! application's own project, so a person sees and operates only what they have been assigned.

use std::collections::BTreeMap;

use apelora_auth::{EnvKind, Resource};
use apelora_domain::lifecycle::{
    Activity, DeploymentStatus, Observed, OperationKind, OperationState, StatusInput,
    derive_status, layers, needs_confirmation,
};
use apelora_domain::plan::{Availability, DesiredModule};
use apelora_domain::{Instance, ProviderMode, is_valid_name};
use apelora_persistence::lifecycle as db;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use super::authz::{allow, audit_scoped, filter_for};
use super::deploy::{bad_request, store};
use super::{ApiError, AppState, AuditItem, AuditList, ErrorBody, Principal, document};

type Res<T> = Result<T, ApiError>;

const MAX_NAME: usize = 40;

fn rfc3339(t: time::OffsetDateTime) -> String {
    t.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

// ---- DTOs -------------------------------------------------------------------------------------

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRef {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub organization_id: Uuid,
    pub organization_slug: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentRef {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub kind: EnvKind,
    pub incus_project: Option<String>,
}

#[derive(Serialize, Clone, Copy, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum DesignStatus {
    /// Being edited; never validated, or invalid.
    Draft,
    /// The current draft passes the server's structural checks.
    Validated,
    /// A version was released from the current draft and is immutable.
    Published,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DesignInfo {
    pub status: DesignStatus,
    pub draft_version: Option<i32>,
    pub validation_errors: i32,
    pub published_version: Option<i32>,
    /// The draft has changes that no published version contains.
    pub has_unpublished_changes: bool,
    pub updated_at: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReplicaSummary {
    pub desired: u32,
    pub running: u32,
    pub healthy: u32,
    pub unhealthy: u32,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OperationSummary {
    pub id: Uuid,
    pub kind: OperationKind,
    pub state: OperationState,
    pub created_at: String,
    pub error: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentInfo {
    pub status: DeploymentStatus,
    /// When the control plane last observed Incus for this answer (RFC 3339).
    pub observed_at: String,
    /// The answer could not be refreshed (Incus unreachable); the status is `unknown`.
    pub stale: bool,
    pub source: ProviderMode,
    /// The published version the application was last deployed from.
    pub deployed_version: Option<i32>,
    pub modules: u32,
    pub replicas: ReplicaSummary,
    pub last_activity_at: Option<String>,
    pub operation: Option<OperationSummary>,
}

#[derive(Serialize, ToSchema)]
pub struct ActionState {
    pub allowed: bool,
    /// Why not, in a sentence the interface can show.
    pub reason: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct Actions {
    pub start: ActionState,
    pub stop: ActionState,
    pub restart: ActionState,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationInfo {
    pub name: String,
    pub latest_revision: Option<i32>,
    /// RFC 3339.
    pub created_at: String,
    pub updated_at: String,
    pub description: String,
    /// Key of the icon (a catalog icon such as `nginx`, or a generic one such as `generic-box`).
    pub icon: String,
    pub project: ProjectRef,
    pub environment: Option<EnvironmentRef>,
    pub archived: bool,
    pub design: DesignInfo,
    pub deployment: DeploymentInfo,
    /// What the caller may do now, from their permissions and the application's state.
    pub actions: Actions,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationList {
    pub items: Vec<ApplicationInfo>,
    /// When Incus was observed for these statuses (RFC 3339), and whether that failed.
    pub observed_at: String,
    pub stale: bool,
}

#[derive(Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct ListQuery {
    #[serde(default)]
    pub include_archived: bool,
}

// ---- derived state ----------------------------------------------------------------------------

struct Observation {
    instances: Option<Vec<Instance>>,
    at: time::OffsetDateTime,
    mode: ProviderMode,
}

async fn observe(state: &AppState) -> Observation {
    Observation {
        instances: state.provider.instances().await.ok(),
        at: time::OffsetDateTime::now_utc(),
        mode: state.provider.mode(),
    }
}

pub(super) fn resource_of(r: &db::ApplicationRow) -> Resource {
    Resource::application(
        r.organization_id,
        r.project_id,
        r.id,
        db::env_kind(&r.environment_kind),
    )
}

pub(super) fn desired_of(rec: &apelora_persistence::DeploymentRecord) -> Vec<DesiredModule> {
    serde_json::from_value(rec.desired.clone()).unwrap_or_default()
}

/// The deployment that belongs to the application's environment (or the only one).
pub(super) fn pick_deployment(
    row: &db::ApplicationRow,
    all: &[apelora_persistence::DeploymentRecord],
) -> Option<apelora_persistence::DeploymentRecord> {
    all.iter()
        .find(|d| row.incus_project.as_deref() == Some(d.project.as_str()))
        .or_else(|| all.first())
        .cloned()
}

fn activity(op: &db::OperationRow) -> Option<Activity> {
    Some(Activity {
        kind: OperationKind::parse(&op.kind)?,
        state: OperationState::parse(&op.state)?,
    })
}

fn replicas_of(desired: &[DesiredModule]) -> Vec<(String, u8)> {
    desired
        .iter()
        .map(|d| (d.module.clone(), d.replicas))
        .collect()
}

fn observed_for(
    name: &str,
    project: &str,
    instances: &[Instance],
    unhealthy: &dyn Fn(&str) -> bool,
) -> Vec<Observed> {
    instances
        .iter()
        .filter(|i| {
            i.managed
                && i.project == project
                && i.owner.as_ref().is_some_and(|o| o.application == name)
        })
        .map(|i| Observed {
            module: i
                .owner
                .as_ref()
                .map(|o| o.module.clone())
                .unwrap_or_default(),
            instance: i.name.clone(),
            status: i.status.clone(),
            unhealthy: unhealthy(&i.name),
            unreachable: i.unreachable,
        })
        .collect()
}

fn design_of(r: &db::ApplicationRow) -> DesignInfo {
    let errors = r.draft_errors.unwrap_or(0);
    let published_current = r.published_revision.is_some()
        && (r.draft_version.is_none() || r.published_from_draft == r.draft_version);
    let status = if published_current {
        DesignStatus::Published
    } else if r.draft_version.is_some() && errors == 0 {
        DesignStatus::Validated
    } else {
        DesignStatus::Draft
    };
    DesignInfo {
        status,
        draft_version: r.draft_version,
        validation_errors: errors,
        published_version: r.published_revision,
        has_unpublished_changes: r.draft_version.is_some() && !published_current,
        updated_at: r.draft_updated_at.map(rfc3339),
    }
}

fn action_states(
    status: DeploymentStatus,
    running: u32,
    may_operate: bool,
    deployed: bool,
) -> Actions {
    let deny = |why: &str| ActionState {
        allowed: false,
        reason: Some(why.to_owned()),
    };
    let ok = || ActionState {
        allowed: true,
        reason: None,
    };
    if !may_operate {
        let why = "you may not start or stop this application";
        return Actions {
            start: deny(why),
            stop: deny(why),
            restart: deny(why),
        };
    }
    if !deployed || status == DeploymentStatus::NotDeployed {
        let why = "deploy a published version first";
        return Actions {
            start: deny(why),
            stop: deny(why),
            restart: deny(why),
        };
    }
    match status {
        DeploymentStatus::Starting | DeploymentStatus::Stopping => {
            let why = "an operation is in progress";
            Actions {
                start: deny(why),
                stop: deny(why),
                restart: deny(why),
            }
        }
        DeploymentStatus::Unknown => {
            let why = "the control plane cannot observe this application right now";
            Actions {
                start: deny(why),
                stop: deny(why),
                restart: deny(why),
            }
        }
        // Everything up and healthy: only Stop.
        DeploymentStatus::Running => Actions {
            start: deny("already running"),
            stop: ok(),
            restart: deny("running normally; stop it first, or restart only when it is degraded"),
        },
        // Some running, something wrong: Restart to recover, or Stop; not Start.
        DeploymentStatus::Degraded | DeploymentStatus::Failed if running > 0 => Actions {
            start: deny("some instances are already running; restart to recover"),
            stop: ok(),
            restart: ok(),
        },
        // Nothing running: only Start.
        DeploymentStatus::Stopped | DeploymentStatus::Degraded | DeploymentStatus::Failed => {
            Actions {
                start: ok(),
                stop: deny("nothing is running"),
                restart: deny("nothing is running"),
            }
        }
        DeploymentStatus::NotDeployed => unreachable_actions(),
    }
}

fn unreachable_actions() -> Actions {
    let d = || ActionState {
        allowed: false,
        reason: Some("deploy a published version first".into()),
    };
    Actions {
        start: d(),
        stop: d(),
        restart: d(),
    }
}

#[allow(clippy::too_many_arguments)]
async fn build_info(
    state: &AppState,
    p: &Principal,
    pool: &apelora_persistence::Pool,
    row: db::ApplicationRow,
    obs: &Observation,
    deployments: &[apelora_persistence::DeploymentRecord],
    latest_op: Option<&db::OperationRow>,
    deployed_at: Option<time::OffsetDateTime>,
) -> Res<ApplicationInfo> {
    let _ = state;
    let dep = pick_deployment(&row, deployments);
    let desired = dep.as_ref().map(desired_of).unwrap_or_default();
    let want = replicas_of(&desired);
    let health = apelora_persistence::application_health(pool, &row.name).await?;
    let unhealthy = |instance: &str| {
        health
            .iter()
            .any(|h| h.instance == instance && h.health == "unhealthy")
    };
    let observed: Option<Vec<Observed>> = match (&obs.instances, &dep) {
        (Some(all), Some(d)) => Some(observed_for(&row.name, &d.project, all, &unhealthy)),
        (Some(_), None) => Some(Vec::new()),
        (None, _) => None,
    };
    let act = latest_op.and_then(activity);
    let status = derive_status(&StatusInput {
        desired: &want,
        observed: observed.as_deref(),
        latest_operation: act,
    });
    let running = observed.as_ref().map_or(0, |o| {
        o.iter().filter(|i| i.status == "Running").count() as u32
    });
    let unhealthy_n = observed.as_ref().map_or(0, |o| {
        o.iter()
            .filter(|i| i.status == "Running" && i.unhealthy)
            .count() as u32
    });
    let may_operate = allow(p, "application.operate", &resource_of(&row)).is_ok();
    let actions = action_states(status, running, may_operate, dep.is_some());
    let design = design_of(&row);
    let modules = if desired.is_empty() {
        0
    } else {
        desired.len() as u32
    };
    Ok(ApplicationInfo {
        name: row.name.clone(),
        latest_revision: row.latest_revision,
        created_at: rfc3339(row.created_at),
        updated_at: rfc3339(row.updated_at),
        description: row.description.clone(),
        icon: row.icon.clone(),
        project: ProjectRef {
            id: row.project_id,
            slug: row.project_slug.clone(),
            name: row.project_name.clone(),
            organization_id: row.organization_id,
            organization_slug: row.organization_slug.clone(),
        },
        environment: row.environment_id.map(|id| EnvironmentRef {
            id,
            slug: row.environment_slug.clone().unwrap_or_default(),
            name: row.environment_name.clone().unwrap_or_default(),
            kind: db::env_kind(&row.environment_kind).unwrap_or(EnvKind::Other),
            incus_project: row.incus_project.clone(),
        }),
        archived: row.archived,
        design,
        deployment: DeploymentInfo {
            status,
            observed_at: rfc3339(obs.at),
            stale: obs.instances.is_none(),
            source: obs.mode,
            deployed_version: dep.as_ref().map(|d| d.revision),
            modules,
            replicas: ReplicaSummary {
                desired: want.iter().map(|(_, n)| u32::from(*n)).sum(),
                running,
                healthy: running.saturating_sub(unhealthy_n),
                unhealthy: unhealthy_n,
            },
            last_activity_at: [
                deployed_at,
                latest_op.map(|o| o.finished_at.unwrap_or(o.created_at)),
            ]
            .into_iter()
            .flatten()
            .max()
            .map(rfc3339),
            operation: latest_op.map(|o| OperationSummary {
                id: o.id,
                kind: OperationKind::parse(&o.kind).unwrap_or(OperationKind::Start),
                state: OperationState::parse(&o.state).unwrap_or(OperationState::Failed),
                created_at: rfc3339(o.created_at),
                error: o.error.clone(),
            }),
        },
        actions,
    })
}

/// A row the caller may view, with the resource it stands for. Unknown or invisible: 404.
pub(super) async fn load(
    state: &AppState,
    p: &Principal,
    name: &str,
) -> Res<(db::ApplicationRow, Resource)> {
    if !is_valid_name(name) {
        return Err(bad_request(format!("invalid application name {name:?}")));
    }
    let pool = store(state)?;
    let missing = || {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            format!("not found: application {name}"),
        )
    };
    let row = db::application_row(pool, name).await?.ok_or_else(missing)?;
    let res = resource_of(&row);
    if allow(p, "application.view", &res).is_err() {
        return Err(missing());
    }
    Ok((row, res))
}

// ---- list and detail --------------------------------------------------------------------------

/// Applications the caller may see, each with design status, derived runtime status, replica
/// health, and the actions they may take now. Needs `application.view`.
#[utoipa::path(get, path = "/api/v1/applications", tag = "applications", operation_id = "applications", params(ListQuery),
    responses((status = 200, body = ApplicationList), (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn list(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Query(q): Query<ListQuery>,
) -> Res<Json<ApplicationList>> {
    let filter = filter_for(&p, "application.view");
    if filter.is_empty() {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            "you may not see applications (application.view)",
        ));
    }
    let pool = store(&state)?;
    let rows = db::list_application_rows(pool, &filter, q.include_archived).await?;
    let obs = observe(&state).await;
    let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
    let ops: BTreeMap<Uuid, db::OperationRow> = db::latest_operations_by_application(pool, &ids)
        .await?
        .into_iter()
        .map(|o| (o.application_id, o))
        .collect();
    let deployed: BTreeMap<Uuid, time::OffsetDateTime> = db::latest_deployed_at(pool, &ids)
        .await?
        .into_iter()
        .collect();
    let all_deployments = apelora_persistence::list_deployments(pool).await?;
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        let mine: Vec<apelora_persistence::DeploymentRecord> = all_deployments
            .iter()
            .filter(|d| d.application_id == row.id)
            .cloned()
            .collect();
        let (op, at) = (ops.get(&row.id), deployed.get(&row.id).copied());
        items.push(build_info(&state, &p, pool, row, &obs, &mine, op, at).await?);
    }
    Ok(Json(ApplicationList {
        items,
        observed_at: rfc3339(obs.at),
        stale: obs.instances.is_none(),
    }))
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InstanceDetail {
    pub instance: String,
    pub status: String,
    pub location: String,
    pub unhealthy: bool,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ModuleDetail {
    pub module: String,
    pub replicas: u8,
    pub availability: Availability,
    pub instances: Vec<InstanceDetail>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct VersionInfo {
    pub revision: i32,
    pub published: bool,
    pub label: Option<String>,
    pub draft_version: Option<i32>,
    pub created_by: String,
    pub created_at: String,
}

#[derive(Serialize, ToSchema)]
pub struct ApplicationDetail {
    pub application: ApplicationInfo,
    pub modules: Vec<ModuleDetail>,
    pub versions: Vec<VersionInfo>,
    pub operations: Vec<OperationDto>,
}

fn version_info(v: db::VersionRow) -> VersionInfo {
    VersionInfo {
        revision: v.revision,
        published: v.published,
        label: v.label,
        draft_version: v.draft_version,
        created_by: v.created_by,
        created_at: rfc3339(v.created_at),
    }
}

/// One application with its modules (desired and observed), versions and recent operations.
/// Needs `application.view`.
#[utoipa::path(get, path = "/api/v1/applications/{name}", tag = "applications", operation_id = "application",
    params(("name" = String, Path, description = "Application name")),
    responses((status = 200, body = ApplicationDetail), (status = 401, body = ErrorBody), (status = 404, body = ErrorBody)))]
pub(super) async fn detail(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(name): Path<String>,
) -> Res<Json<ApplicationDetail>> {
    let (row, _) = load(&state, &p, &name).await?;
    let pool = store(&state)?;
    let obs = observe(&state).await;
    let deployments = db::deployments_of(pool, row.id).await?;
    let ops = db::list_operations(pool, row.id, 5).await?;
    let deployed_at = db::latest_deployed_at(pool, &[row.id])
        .await?
        .into_iter()
        .next()
        .map(|(_, t)| t);
    let dep = pick_deployment(&row, &deployments);
    let health = apelora_persistence::application_health(pool, &row.name).await?;
    let modules = match (&dep, &obs.instances) {
        (Some(d), instances) => desired_of(d)
            .into_iter()
            .map(|a| ModuleDetail {
                instances: instances
                    .iter()
                    .flatten()
                    .filter(|i| {
                        i.managed
                            && i.project == d.project
                            && i.owner
                                .as_ref()
                                .is_some_and(|o| o.application == row.name && o.module == a.module)
                    })
                    .map(|i| InstanceDetail {
                        instance: i.name.clone(),
                        status: i.status.clone(),
                        location: i.location.clone(),
                        unhealthy: i.status == "Running"
                            && health
                                .iter()
                                .any(|h| h.instance == i.name && h.health == "unhealthy"),
                    })
                    .collect(),
                module: a.module,
                replicas: a.replicas,
                availability: a.availability,
            })
            .collect(),
        (None, _) => Vec::new(),
    };
    let info = build_info(
        &state,
        &p,
        pool,
        row.clone(),
        &obs,
        &deployments,
        ops.first(),
        deployed_at,
    )
    .await?;
    Ok(Json(ApplicationDetail {
        application: info,
        modules,
        versions: db::list_versions(pool, row.id)
            .await?
            .into_iter()
            .map(version_info)
            .collect(),
        operations: ops.into_iter().map(operation_dto).collect(),
    }))
}

// ---- create and describe ----------------------------------------------------------------------

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateApplication {
    /// Lowercase letters, digits and hyphens, starting with a letter; at most 40 characters. Fixed
    /// for the life of the application (instances are named after it).
    pub name: String,
    /// The environment (and so the project) the application lives in.
    pub environment_id: Uuid,
    #[serde(default)]
    pub description: String,
    /// Icon key; `box` when omitted.
    pub icon: Option<String>,
    /// The first draft (a Studio document), if any.
    #[schema(value_type = Option<Object>)]
    pub document: Option<Value>,
}

fn valid_icon(s: &str) -> bool {
    (1..=40).contains(&s.len())
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Creates an application in an environment, optionally with its first draft. Needs
/// `application.create` on the environment's project.
#[utoipa::path(post, path = "/api/v1/applications", tag = "applications", operation_id = "create_application", request_body = CreateApplication,
    responses((status = 201, body = ApplicationInfo), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody), (status = 409, body = ErrorBody)))]
pub(super) async fn create(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    body: Result<Json<CreateApplication>, JsonRejection>,
) -> Res<(StatusCode, Json<ApplicationInfo>)> {
    let Json(req) = body.map_err(|e| bad_request(e.body_text()))?;
    if !is_valid_name(&req.name) || req.name.len() > MAX_NAME {
        return Err(bad_request(format!(
            "an application name is 1–{MAX_NAME} lowercase letters, digits and hyphens, starting with a letter"
        )));
    }
    if req.description.chars().count() > 200 {
        return Err(bad_request("a description is at most 200 characters"));
    }
    let icon = req.icon.as_deref().unwrap_or("box");
    if !valid_icon(icon) {
        return Err(bad_request("an icon is a key such as nginx or generic-box"));
    }
    let pool = store(&state)?;
    // An unknown environment looks the same as one the caller may not use.
    let Some((env, org)) =
        apelora_persistence::identity::environment_by_id(pool, req.environment_id).await?
    else {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            "you may not create applications there (application.create)",
        ));
    };
    let (project, kind) = (env.project_id, EnvKind::parse(&env.kind));
    let res = Resource {
        organization: Some(org),
        project: Some(project),
        environment: kind,
        ..Resource::default()
    };
    allow(&p, "application.create", &res)?;
    if let Some(doc) = &req.document
        && serde_json::to_vec(doc).map_or(0, |b| b.len()) > document::MAX_BYTES
    {
        return Err(bad_request("the document is larger than 2 MB"));
    }
    let app = db::create_application_full(
        pool,
        &req.name,
        project,
        Some(env.id),
        req.description.trim(),
        icon,
        &p.actor_ref(),
    )
    .await?;
    if let Some(doc) = &req.document {
        let errors = json!(document::validate(doc));
        db::put_draft(pool, app.id, doc, None, &errors, &p.actor_ref())
            .await?
            .map_err(|_| {
                ApiError::new(
                    StatusCode::CONFLICT,
                    "draft_conflict",
                    "the draft changed while it was created",
                )
            })?;
    }
    let scoped = Resource {
        application: Some(app.id),
        ..res
    };
    audit_scoped(
        pool,
        &p,
        "application.create",
        &app.name,
        "success",
        Some(&scoped),
        &json!({ "environment": env.slug, "icon": icon }),
    )
    .await?;
    let row = db::application_row(pool, &app.name)
        .await?
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "not_found", "not found"))?;
    let obs = observe(&state).await;
    Ok((
        StatusCode::CREATED,
        Json(build_info(&state, &p, pool, row, &obs, &[], None, None).await?),
    ))
}

#[derive(Deserialize, ToSchema)]
pub struct UpdateApplication {
    pub description: Option<String>,
    pub icon: Option<String>,
}

/// Changes an application's description or icon (its name cannot change). Needs `application.design`.
#[utoipa::path(patch, path = "/api/v1/applications/{name}", tag = "applications", operation_id = "update_application", request_body = UpdateApplication,
    params(("name" = String, Path, description = "Application name")),
    responses((status = 200, body = ApplicationInfo), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody)))]
pub(super) async fn update(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(name): Path<String>,
    body: Result<Json<UpdateApplication>, JsonRejection>,
) -> Res<Json<ApplicationInfo>> {
    let (row, res) = load(&state, &p, &name).await?;
    allow(&p, "application.design", &res)?;
    let Json(req) = body.map_err(|e| bad_request(e.body_text()))?;
    if req
        .description
        .as_ref()
        .is_some_and(|d| d.chars().count() > 200)
    {
        return Err(bad_request("a description is at most 200 characters"));
    }
    if req.icon.as_deref().is_some_and(|i| !valid_icon(i)) {
        return Err(bad_request("an icon is a key such as nginx or generic-box"));
    }
    let pool = store(&state)?;
    db::update_application_meta(
        pool,
        row.id,
        req.description.as_deref().map(str::trim),
        req.icon.as_deref(),
    )
    .await?;
    audit_scoped(
        pool,
        &p,
        "application.update",
        &name,
        "success",
        Some(&res),
        &json!({ "icon": req.icon, "descriptionChanged": req.description.is_some() }),
    )
    .await?;
    let fresh = db::application_row(pool, &name)
        .await?
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "not_found", "not found"))?;
    let obs = observe(&state).await;
    let deployments = db::deployments_of(pool, fresh.id).await?;
    let op = db::list_operations(pool, fresh.id, 1).await?;
    Ok(Json(
        build_info(
            &state,
            &p,
            pool,
            fresh,
            &obs,
            &deployments,
            op.first(),
            None,
        )
        .await?,
    ))
}

// ---- drafts and versions ----------------------------------------------------------------------

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DraftDto {
    pub version: i32,
    #[schema(value_type = Object)]
    pub document: Value,
    pub validation_errors: Vec<String>,
    pub updated_by: String,
    pub updated_at: String,
}

fn draft_dto(d: db::DraftRow) -> DraftDto {
    let errors = d
        .validation_errors
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    DraftDto {
        version: d.version,
        document: d.document,
        validation_errors: errors,
        updated_by: d.updated_by,
        updated_at: rfc3339(d.updated_at),
    }
}

/// The application's working document. 404 when it has none yet. Needs `application.view`.
#[utoipa::path(get, path = "/api/v1/applications/{name}/draft", tag = "applications", operation_id = "application_draft",
    params(("name" = String, Path, description = "Application name")),
    responses((status = 200, body = DraftDto), (status = 404, body = ErrorBody)))]
pub(super) async fn get_draft(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(name): Path<String>,
) -> Res<Json<DraftDto>> {
    let (row, _) = load(&state, &p, &name).await?;
    let d = db::get_draft(store(&state)?, row.id)
        .await?
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "no_draft",
                format!("{name} has no draft yet"),
            )
        })?;
    Ok(Json(draft_dto(d)))
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SaveDraft {
    #[schema(value_type = Object)]
    pub document: Value,
    /// The draft version this edit started from; omit only for the first save.
    pub base_version: Option<i32>,
}

/// Saves the draft as the next version. Refused with 409 `draft_conflict` (and the current
/// version) when someone saved since `baseVersion`; nothing is overwritten. Needs `application.design`.
#[utoipa::path(put, path = "/api/v1/applications/{name}/draft", tag = "applications", operation_id = "save_application_draft", request_body = SaveDraft,
    params(("name" = String, Path, description = "Application name")),
    responses((status = 200, body = DraftDto), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody), (status = 409, body = ErrorBody)))]
pub(super) async fn put_draft(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(name): Path<String>,
    body: Result<Json<SaveDraft>, JsonRejection>,
) -> Res<Response> {
    let (row, res) = load(&state, &p, &name).await?;
    allow(&p, "application.design", &res)?;
    let Json(req) = body.map_err(|e| bad_request(e.body_text()))?;
    if serde_json::to_vec(&req.document).map_or(usize::MAX, |b| b.len()) > document::MAX_BYTES {
        return Err(bad_request("the document is larger than 2 MB"));
    }
    let errors = json!(document::validate(&req.document));
    let pool = store(&state)?;
    match db::put_draft(pool, row.id, &req.document, req.base_version, &errors, &p.actor_ref()).await? {
        Ok(d) => {
            audit_scoped(pool, &p, "application.draft.save", &name, "success", Some(&res), &json!({ "version": d.version, "errors": errors.as_array().map_or(0, Vec::len) })).await?;
            Ok(Json(draft_dto(d)).into_response())
        }
        Err(current) => Ok((StatusCode::CONFLICT, Json(json!({ "error": { "code": "draft_conflict", "message": format!("someone saved version {current} since you started from {}; nothing was overwritten", req.base_version.map_or("the beginning".to_owned(), |v| v.to_string())) }, "currentVersion": current }))).into_response()),
    }
}

#[derive(Deserialize, ToSchema)]
pub struct PublishRequest {
    /// A short name for the version, e.g. `after load test`.
    pub label: Option<String>,
}

/// Releases the current draft as the next immutable published version. Refused while the draft
/// is invalid or when this draft version is already published. Needs `application.design`.
#[utoipa::path(post, path = "/api/v1/applications/{name}/versions", tag = "applications", operation_id = "publish_application", request_body = PublishRequest,
    params(("name" = String, Path, description = "Application name")),
    responses((status = 201, body = VersionInfo), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody), (status = 422, body = ErrorBody)))]
pub(super) async fn publish(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(name): Path<String>,
    body: Result<Json<PublishRequest>, JsonRejection>,
) -> Res<(StatusCode, Json<VersionInfo>)> {
    let (row, res) = load(&state, &p, &name).await?;
    allow(&p, "application.design", &res)?;
    let Json(req) = body.map_err(|e| bad_request(e.body_text()))?;
    if req.label.as_ref().is_some_and(|l| l.chars().count() > 80) {
        return Err(bad_request("a label is at most 80 characters"));
    }
    let pool = store(&state)?;
    match db::publish_draft(
        pool,
        row.id,
        req.label
            .as_deref()
            .map(str::trim)
            .filter(|l| !l.is_empty()),
        &p.actor_ref(),
    )
    .await?
    {
        Ok(v) => {
            audit_scoped(
                pool,
                &p,
                "application.version.publish",
                &name,
                "success",
                Some(&res),
                &json!({ "revision": v.revision, "draftVersion": v.draft_version }),
            )
            .await?;
            Ok((StatusCode::CREATED, Json(version_info(v))))
        }
        Err(db::PublishRefusal::NoDraft) => Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "no_draft",
            "there is no draft to publish",
        )),
        Err(db::PublishRefusal::AlreadyPublished(r)) => Err(ApiError::new(
            StatusCode::CONFLICT,
            "already_published",
            format!("this draft is already published as version {r}; change it to publish again"),
        )),
        Err(db::PublishRefusal::Invalid(errors)) => Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "draft_invalid",
            format!(
                "the draft has problems: {}",
                errors
                    .as_array()
                    .map(|a| a
                        .iter()
                        .filter_map(Value::as_str)
                        .take(5)
                        .collect::<Vec<_>>()
                        .join("; "))
                    .unwrap_or_default()
            ),
        )),
    }
}

#[derive(Serialize, ToSchema)]
pub struct VersionList {
    pub items: Vec<VersionInfo>,
}

/// Versions of the application, newest first. Needs `application.view`.
#[utoipa::path(get, path = "/api/v1/applications/{name}/versions", tag = "applications", operation_id = "application_versions",
    params(("name" = String, Path, description = "Application name")),
    responses((status = 200, body = VersionList), (status = 404, body = ErrorBody)))]
pub(super) async fn versions(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(name): Path<String>,
) -> Res<Json<VersionList>> {
    let (row, _) = load(&state, &p, &name).await?;
    Ok(Json(VersionList {
        items: db::list_versions(store(&state)?, row.id)
            .await?
            .into_iter()
            .map(version_info)
            .collect(),
    }))
}

#[derive(Serialize, ToSchema)]
pub struct VersionDocument {
    #[schema(value_type = Object)]
    pub document: Value,
    pub published: bool,
}

/// The document of one version. Needs `application.view`.
#[utoipa::path(get, path = "/api/v1/applications/{name}/versions/{revision}", tag = "applications", operation_id = "application_version",
    params(("name" = String, Path, description = "Application name"), ("revision" = i32, Path, description = "Version number")),
    responses((status = 200, body = VersionDocument), (status = 404, body = ErrorBody)))]
pub(super) async fn version(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path((name, revision)): Path<(String, i32)>,
) -> Res<Json<VersionDocument>> {
    let (row, _) = load(&state, &p, &name).await?;
    let (document, published) = db::version_document(store(&state)?, row.id, revision)
        .await?
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "not_found",
                format!("not found: version {revision}"),
            )
        })?;
    Ok(Json(VersionDocument {
        document,
        published,
    }))
}

/// Archives an application that is not running; it leaves the default list and keeps its data.
/// Needs `application.delete`.
#[utoipa::path(post, path = "/api/v1/applications/{name}/archive", tag = "applications", operation_id = "archive_application",
    params(("name" = String, Path, description = "Application name")),
    responses((status = 204), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody)))]
pub(super) async fn archive(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(name): Path<String>,
) -> Res<StatusCode> {
    set_archived(&state, &p, &name, true).await
}

/// Brings an archived application back. Needs `application.delete`.
#[utoipa::path(post, path = "/api/v1/applications/{name}/restore", tag = "applications", operation_id = "restore_application",
    params(("name" = String, Path, description = "Application name")),
    responses((status = 204), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody)))]
pub(super) async fn restore(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(name): Path<String>,
) -> Res<StatusCode> {
    set_archived(&state, &p, &name, false).await
}

async fn set_archived(
    state: &AppState,
    p: &Principal,
    name: &str,
    archived: bool,
) -> Res<StatusCode> {
    let (row, res) = load(state, p, name).await?;
    allow(p, "application.delete", &res)?;
    let pool = store(state)?;
    if archived {
        let obs = observe(state).await;
        let deployments = db::deployments_of(pool, row.id).await?;
        let info = build_info(state, p, pool, row.clone(), &obs, &deployments, None, None).await?;
        if !matches!(
            info.deployment.status,
            DeploymentStatus::NotDeployed | DeploymentStatus::Stopped
        ) {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "still_running",
                "stop the application before archiving it",
            ));
        }
    }
    db::set_archived(pool, row.id, archived).await?;
    audit_scoped(
        pool,
        p,
        if archived {
            "application.archive"
        } else {
            "application.restore"
        },
        name,
        "success",
        Some(&res),
        &json!({}),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---- operations -------------------------------------------------------------------------------

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StepDto {
    pub instance: String,
    pub module: String,
    /// `pending`, `running`, `succeeded`, `failed` or `skipped`.
    pub state: String,
    pub error: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct PhaseDto {
    /// `start` or `stop`.
    pub action: String,
    pub state: String,
    pub steps: Vec<StepDto>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OperationDto {
    pub id: Uuid,
    pub kind: OperationKind,
    pub state: OperationState,
    pub requested_by: String,
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub error: Option<String>,
    pub phases: Vec<PhaseDto>,
    pub steps_done: u32,
    pub steps_total: u32,
}

fn operation_dto(o: db::OperationRow) -> OperationDto {
    let phases: Vec<PhaseDto> = o
        .phases
        .as_array()
        .map(|a| {
            a.iter()
                .map(|ph| PhaseDto {
                    action: ph["action"].as_str().unwrap_or("").to_owned(),
                    state: ph["state"].as_str().unwrap_or("").to_owned(),
                    steps: ph["steps"]
                        .as_array()
                        .map(|s| {
                            s.iter()
                                .map(|st| StepDto {
                                    instance: st["name"].as_str().unwrap_or("").to_owned(),
                                    // Operations recorded before the rename name the step's `appliance`.
                                    module: st["module"]
                                        .as_str()
                                        .or_else(|| st["appliance"].as_str())
                                        .unwrap_or("")
                                        .to_owned(),
                                    state: st["state"].as_str().unwrap_or("").to_owned(),
                                    error: st["error"].as_str().map(str::to_owned),
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default();
    let steps_total = phases.iter().map(|p| p.steps.len() as u32).sum();
    let steps_done = phases
        .iter()
        .flat_map(|p| &p.steps)
        .filter(|s| s.state == "succeeded")
        .count() as u32;
    OperationDto {
        id: o.id,
        kind: OperationKind::parse(&o.kind).unwrap_or(OperationKind::Start),
        state: OperationState::parse(&o.state).unwrap_or(OperationState::Failed),
        requested_by: o.requested_by,
        created_at: rfc3339(o.created_at),
        started_at: o.started_at.map(rfc3339),
        finished_at: o.finished_at.map(rfc3339),
        error: o.error,
        phases,
        steps_done,
        steps_total,
    }
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OperateRequest {
    pub action: OperationKind,
    /// Required to stop or restart an application that has single-instance or stateful
    /// modules: it acknowledges that their clients lose service meanwhile.
    #[serde(default)]
    pub confirm_disruption: bool,
}

#[derive(Serialize, ToSchema)]
pub struct OperationAccepted {
    pub operation: OperationDto,
    /// False when the same `Idempotency-Key` had already created it.
    pub created: bool,
}

/// Starts, stops or restarts the application's instances, in dependency order, as an
/// asynchronous operation. Succeeds only when Incus confirms each instance's state. Needs
/// `application.operate` on the application. It only touches instances this application owns;
/// networks, volumes, data and other applications are left alone.
#[utoipa::path(post, path = "/api/v1/applications/{name}/operations", tag = "applications", operation_id = "operate_application", request_body = OperateRequest,
    params(("name" = String, Path, description = "Application name"),
        ("Idempotency-Key" = Option<String>, Header, description = "Makes the request safe to repeat")),
    responses((status = 202, body = OperationAccepted), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody),
        (status = 404, body = ErrorBody), (status = 409, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn operate(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(name): Path<String>,
    headers: HeaderMap,
    body: Result<Json<OperateRequest>, JsonRejection>,
) -> Res<(StatusCode, Json<OperationAccepted>)> {
    let (row, res) = load(&state, &p, &name).await?;
    allow(&p, "application.operate", &res)?;
    let Json(req) = body.map_err(|e| bad_request(e.body_text()))?;
    let pool = store(&state)?;
    let conflict = |code: &'static str, msg: String| ApiError::new(StatusCode::CONFLICT, code, msg);

    let deployments = db::deployments_of(pool, row.id).await?;
    let Some(dep) = pick_deployment(&row, &deployments) else {
        return Err(conflict(
            "not_deployed",
            "this application has no deployed version; deploy a published version first".into(),
        ));
    };
    if let Some(active) = db::list_operations(pool, row.id, 1)
        .await?
        .first()
        .filter(|o| OperationState::parse(&o.state).is_some_and(OperationState::is_active))
    {
        return Err(conflict(
            "operation_in_progress",
            format!("a {} operation is still in progress", active.kind),
        ));
    }
    // Never act on a guess: if Incus cannot be read, refuse.
    let Ok(all) = state.provider.instances().await else {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "incus_unreachable",
            "the control plane cannot reach Incus, so it cannot check the application's state; nothing was changed",
        ));
    };
    let desired = desired_of(&dep);
    let mine: Vec<&Instance> = all
        .iter()
        .filter(|i| {
            i.managed
                && i.project == dep.project
                && i.owner.as_ref().is_some_and(|o| o.application == name)
        })
        .collect();
    if let Some(bad) = mine.iter().find(|i| i.unreachable) {
        return Err(conflict(
            "member_unreachable",
            format!(
                "{} is on a cluster member that cannot be reached; nothing was changed",
                bad.name
            ),
        ));
    }
    let running = mine.iter().filter(|i| i.status == "Running").count();
    match req.action {
        OperationKind::Start if mine.is_empty() => {
            return Err(conflict(
                "nothing_deployed",
                "none of the application's instances exist in Incus; deploy it again".into(),
            ));
        }
        OperationKind::Start if running == mine.len() => {
            return Err(conflict(
                "already_running",
                "every instance is already running".into(),
            ));
        }
        OperationKind::Stop | OperationKind::Restart if running == 0 => {
            return Err(conflict("already_stopped", "nothing is running".into()));
        }
        _ => {}
    }

    // The same rules the interface shows: an action that is greyed out is refused here too.
    let observed_now = Observation {
        instances: Some(all.clone()),
        at: time::OffsetDateTime::now_utc(),
        mode: state.provider.mode(),
    };
    let ops_now = db::list_operations(pool, row.id, 1).await?;
    let info = build_info(
        &state,
        &p,
        pool,
        row.clone(),
        &observed_now,
        &deployments,
        ops_now.first(),
        None,
    )
    .await?;
    let verdict = match req.action {
        OperationKind::Start => &info.actions.start,
        OperationKind::Stop => &info.actions.stop,
        OperationKind::Restart => &info.actions.restart,
    };
    if !verdict.allowed {
        return Err(conflict(
            "action_not_available",
            format!(
                "{} is not available now: {}",
                req.action.as_str(),
                verdict.reason.clone().unwrap_or_default()
            ),
        ));
    }

    // Stopping interrupts clients of single-instance and stateful modules: ask for confirmation.
    if matches!(req.action, OperationKind::Stop | OperationKind::Restart) && !req.confirm_disruption
    {
        let at_risk: Vec<(String, Availability)> = needs_confirmation(&desired)
            .into_iter()
            .filter(|(a, _)| {
                mine.iter().any(|i| {
                    i.status == "Running" && i.owner.as_ref().is_some_and(|o| &o.module == a)
                })
            })
            .collect();
        if !at_risk.is_empty() {
            let names = at_risk
                .iter()
                .map(|(a, k)| {
                    format!(
                        "{a} ({})",
                        match k {
                            Availability::Stateful => "stateful",
                            _ => "single instance",
                        }
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            return Err(conflict(
                "confirmation_required",
                format!(
                    "{} will interrupt service for clients of {names}. Send confirmDisruption to proceed; data is kept.",
                    req.action.as_str()
                ),
            ));
        }
    }

    // Phases: dependency order from the recorded connections.
    // Unreadable connections leave every module in one phase: noticeable, so logged.
    let connections: Vec<apelora_domain::network::Connection> = dep
        .connections
        .clone()
        .and_then(|c| {
            serde_json::from_value(c)
                .map_err(|e| tracing::warn!(application = %name, error = %e, "operation: the deployment's connections are unreadable; phases ignore dependency order"))
                .ok()
        })
        .unwrap_or_default();
    let modules: Vec<String> = desired.iter().map(|d| d.module.clone()).collect();
    let phase = |action: &str,
                 order: Vec<Vec<String>>,
                 wanted: &dyn Fn(&Instance) -> bool|
     -> Vec<Value> {
        order
            .into_iter()
            .filter_map(|layer| {
                let steps: Vec<Value> = layer
                    .iter()
                    .flat_map(|a| mine.iter().filter(move |i| i.owner.as_ref().is_some_and(|o| &o.module == a) && wanted(i)))
                    .map(|i| json!({ "project": i.project, "name": i.name, "module": i.owner.as_ref().map(|o| o.module.clone()).unwrap_or_default(), "state": "pending" }))
                    .collect();
                (!steps.is_empty()).then(|| json!({ "action": action, "state": "pending", "steps": steps }))
            })
            .collect()
    };
    let up = |i: &Instance| i.status == "Running";
    let down = |i: &Instance| i.status != "Running";
    let phases: Vec<Value> = match req.action {
        OperationKind::Start => phase("start", layers(&modules, &connections, true), &down),
        OperationKind::Stop => phase("stop", layers(&modules, &connections, false), &up),
        OperationKind::Restart => {
            let mut v = phase("stop", layers(&modules, &connections, false), &up);
            // Everything that was running comes back, providers first.
            let was_running: Vec<String> = mine
                .iter()
                .filter(|i| up(i))
                .map(|i| i.name.clone())
                .collect();
            v.extend(phase(
                "start",
                layers(&modules, &connections, true),
                &|i: &Instance| was_running.contains(&i.name),
            ));
            v
        }
    };
    if phases.is_empty() {
        return Err(conflict(
            "nothing_to_do",
            "no instance needs to change".into(),
        ));
    }
    let key = super::deploy::idempotency_key(&headers, &p)?.map(|k| format!("{k}:{name}:operate"));
    let (op, created) = db::create_operation(
        pool,
        &db::NewOperation {
            application_id: row.id,
            project: &dep.project,
            kind: req.action.as_str(),
            phases: &json!(phases),
            requested_by: &p.actor_ref(),
            confirmed: req.confirm_disruption,
            key: key.as_deref(),
        },
    )
    .await
    .map_err(|e| match e {
        apelora_persistence::StoreError::Conflict(m) => {
            ApiError::new(StatusCode::CONFLICT, "operation_in_progress", m)
        }
        other => other.into(),
    })?;
    if created {
        audit_scoped(pool, &p, &format!("application.{}.requested", req.action.as_str()), &name, "success", Some(&res), &json!({ "operation": op.id, "confirmedDisruption": req.confirm_disruption, "instances": phases.iter().map(|p| p["steps"].as_array().map_or(0, Vec::len)).sum::<usize>() })).await?;
    }
    Ok((
        StatusCode::ACCEPTED,
        Json(OperationAccepted {
            operation: operation_dto(op),
            created,
        }),
    ))
}

#[derive(Serialize, ToSchema)]
pub struct OperationList {
    pub items: Vec<OperationDto>,
}

/// Recent operations of the application, newest first. Needs `application.view`.
#[utoipa::path(get, path = "/api/v1/applications/{name}/operations", tag = "applications", operation_id = "application_operations",
    params(("name" = String, Path, description = "Application name")),
    responses((status = 200, body = OperationList), (status = 404, body = ErrorBody)))]
pub(super) async fn operations(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(name): Path<String>,
) -> Res<Json<OperationList>> {
    let (row, _) = load(&state, &p, &name).await?;
    Ok(Json(OperationList {
        items: db::list_operations(store(&state)?, row.id, 20)
            .await?
            .into_iter()
            .map(operation_dto)
            .collect(),
    }))
}

/// One operation with the progress of each phase and instance. Needs `application.view`.
#[utoipa::path(get, path = "/api/v1/applications/{name}/operations/{id}", tag = "applications", operation_id = "application_operation",
    params(("name" = String, Path, description = "Application name"), ("id" = Uuid, Path, description = "Operation id")),
    responses((status = 200, body = OperationDto), (status = 404, body = ErrorBody)))]
pub(super) async fn operation(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path((name, id)): Path<(String, Uuid)>,
) -> Res<Json<OperationDto>> {
    let (row, _) = load(&state, &p, &name).await?;
    let op = db::get_operation(store(&state)?, id)
        .await?
        .filter(|o| o.application_id == row.id)
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "not_found",
                format!("not found: operation {id}"),
            )
        })?;
    Ok(Json(operation_dto(op)))
}

/// What happened to the application: creation, saves, versions, deployments, operations,
/// refusals. Needs `application.view`.
#[utoipa::path(get, path = "/api/v1/applications/{name}/events", tag = "applications", operation_id = "application_events",
    params(("name" = String, Path, description = "Application name")),
    responses((status = 200, body = AuditList), (status = 404, body = ErrorBody)))]
pub(super) async fn events(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(name): Path<String>,
) -> Res<Json<AuditList>> {
    let (row, _) = load(&state, &p, &name).await?;
    let rows = db::application_events(store(&state)?, row.id, 100).await?;
    Ok(Json(AuditList {
        items: rows.into_iter().map(AuditItem::from_row).collect(),
    }))
}
