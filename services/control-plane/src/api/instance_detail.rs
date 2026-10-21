// SPDX-License-Identifier: AGPL-3.0-only
//! One instance in detail, and its logs, read from Incus through the adapter.
//!
//! The detail is shown to whole-platform readers (`cluster.view`), to people who manage the
//! environment that owns the instance's Incus project (`instance.manage`), and to people who may
//! view the application it belongs to (`application.view`); anyone else gets 404, so names do not
//! leak. Logs need `application.logs.read` on its application or `instance.manage` on its
//! environment. Logs are bounded (the last `tailKib` KiB, at most 1 MiB), are named only as
//! Incus lists them (no paths), and are never stored or logged by Apelora.

use apelora_auth::Resource;
use apelora_domain::inspect::{InstanceView, LogTail, log_file_name};
use apelora_domain::{Permission, is_valid_name};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use super::authz::allow;
use super::deploy::{
    InstancePath, Job, authorize_instance, ensure_project_allowed, instance_resource, store,
};
use super::manage::ActionState;
use super::{ApiError, AppState, ErrorBody, Principal, Source, require, source};

/// What the caller may do with the instance, each with the reason when not.
#[derive(Serialize, ToSchema)]
pub struct InstancePageActions {
    pub start: ActionState,
    pub stop: ActionState,
    pub restart: ActionState,
    #[serde(rename = "move")]
    pub move_: ActionState,
    pub delete: ActionState,
    pub logs: ActionState,
}

/// One instance as Incus reports it, with the application it belongs to and its recent jobs.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InstancePage {
    pub source: Source,
    pub instance: InstanceView,
    /// The application it belongs to (its Apelora labels).
    pub application: Option<String>,
    /// The latest jobs for it, newest first (at most 10).
    pub jobs: Vec<Job>,
    pub actions: InstancePageActions,
}

/// The instance's log files.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InstanceLogList {
    pub source: Source,
    /// File names as Incus lists them (e.g. `lxc.log`, `qemu.log`).
    pub files: Vec<String>,
}

/// The end of one log.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InstanceLogText {
    pub source: Source,
    /// The file name, or `console` for the console log.
    pub file: String,
    /// How much of the end was asked for, in KiB.
    pub tail_kib: u32,
    pub log: LogTail,
}

#[derive(Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
pub(super) struct TailQuery {
    /// How many KiB from the end of the log (1–1024, default 64).
    tail_kib: Option<u32>,
}

#[derive(Deserialize, IntoParams)]
pub(super) struct LogPath {
    /// Incus project.
    project: String,
    /// Instance name.
    name: String,
    /// Log file name, as listed.
    file: String,
}

const DEFAULT_TAIL_KIB: u32 = 64;
const MAX_TAIL_KIB: u32 = 1024;

fn not_found(project: &str, name: &str) -> ApiError {
    ApiError::new(
        StatusCode::NOT_FOUND,
        "not_found",
        format!("not found: instance {project}/{name}"),
    )
}

fn check_names(project: &str, name: &str) -> Result<(), ApiError> {
    if is_valid_name(project) && is_valid_name(name) {
        Ok(())
    } else {
        Err(super::deploy::bad_request(
            "invalid project or instance name",
        ))
    }
}

/// The resource of the application an instance belongs to, if Apelora knows it.
async fn application_resource(
    state: &AppState,
    application: Option<&str>,
) -> Result<Option<Resource>, ApiError> {
    let Some(name) = application else {
        return Ok(None);
    };
    Ok(
        apelora_persistence::identity::application_context(store(state)?, name)
            .await?
            .map(|c| c.resource()),
    )
}

/// The instance, if the caller may see it (404 otherwise, as for an instance that does not exist).
async fn visible(
    state: &AppState,
    p: &Principal,
    project: &str,
    name: &str,
) -> Result<(InstanceView, Option<Resource>), ApiError> {
    check_names(project, name)?;
    let view = state
        .provider
        .instance_view(project, name)
        .await?
        .ok_or_else(|| not_found(project, name))?;
    let app = view.owner.as_ref().map(|o| o.application.clone());
    let app_resource = match &state.auth {
        super::Auth::Tokens(_) => application_resource(state, app.as_deref()).await?,
        super::Auth::DisabledForDevelopment => None,
    };
    let seen = require(p, Permission::ReadInventory).is_ok()
        || allow(
            p,
            "instance.manage",
            &instance_resource(state, project).await?,
        )
        .is_ok()
        || app_resource
            .as_ref()
            .is_some_and(|r| allow(p, "application.view", r).is_ok());
    if seen {
        Ok((view, app_resource))
    } else {
        Err(not_found(project, name))
    }
}

/// May the caller read this instance's logs? `application.logs.read` on its application, or
/// `instance.manage` on its environment.
async fn may_read_logs(
    state: &AppState,
    p: &Principal,
    project: &str,
    app: Option<&Resource>,
) -> Result<bool, ApiError> {
    Ok(
        app.is_some_and(|r| allow(p, "application.logs.read", r).is_ok())
            || allow(
                p,
                "instance.manage",
                &instance_resource(state, project).await?,
            )
            .is_ok(),
    )
}

fn yes() -> ActionState {
    ActionState {
        allowed: true,
        reason: None,
    }
}

fn no(reason: impl Into<String>) -> ActionState {
    ActionState {
        allowed: false,
        reason: Some(reason.into()),
    }
}

/// The instance with the application it belongs to, its recent jobs, and what the caller may
/// do (Start, Stop, Restart, Move and Delete go through the existing instance routes and jobs).
#[utoipa::path(get, path = "/api/v1/instances/{project}/{name}", tag = "inventory", operation_id = "instance_page",
    params(InstancePath),
    responses((status = 200, body = InstancePage), (status = 400, body = ErrorBody), (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 502, body = ErrorBody)))]
pub(super) async fn page(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(InstancePath { project, name }): Path<InstancePath>,
) -> Result<Json<InstancePage>, ApiError> {
    let (view, app_resource) = visible(&state, &p, &project, &name).await?;
    let jobs = match &state.auth {
        super::Auth::Tokens(pool) => {
            let rows = apelora_persistence::jobs::instance_jobs(pool, &project, &name, 10).await?;
            let mut visible_rows = Vec::new();
            for r in rows {
                if super::deploy::job_visible(&state, &p, &r).await? {
                    visible_rows.push(r);
                }
            }
            super::jobs::with_actions(&state, &p, visible_rows).await?
        }
        super::Auth::DisabledForDevelopment => Vec::new(),
    };
    let in_flight = jobs
        .iter()
        .find(|j| matches!(j.status.as_str(), "pending" | "running"));
    let change = if !view.managed {
        Err("Not managed by Apelora: Apelora changes only instances it created.".to_owned())
    } else if authorize_instance(&state, &p, &project).await.is_err() {
        Err("You may not change instances in this project (instance.manage).".to_owned())
    } else if ensure_project_allowed(&state, &project).await.is_err() {
        Err(format!(
            "Apelora is not configured to change project {project}."
        ))
    } else if let Some(j) = in_flight {
        Err(format!(
            "A job for this instance is {} ({}).",
            j.status, j.kind
        ))
    } else {
        Ok(())
    };
    let running = view.status == "Running";
    let when = |ok: bool, why: &str| match &change {
        Err(e) => no(e.clone()),
        Ok(()) if ok => yes(),
        Ok(()) => no(why),
    };
    let logs = if may_read_logs(&state, &p, &project, app_resource.as_ref()).await? {
        yes()
    } else {
        no(
            "Reading logs needs application.logs.read on its application or instance.manage on its environment.",
        )
    };
    let actions = InstancePageActions {
        start: when(!running, "It is running."),
        stop: when(running, "It is not running."),
        restart: when(running, "It is not running; start it instead."),
        move_: when(true, ""),
        delete: when(true, ""),
        logs,
    };
    Ok(Json(InstancePage {
        source: source(state.provider.mode()),
        application: view.owner.as_ref().map(|o| o.application.clone()),
        instance: view,
        jobs,
        actions,
    }))
}

async fn log_access(
    state: &AppState,
    p: &Principal,
    project: &str,
    name: &str,
) -> Result<(), ApiError> {
    let (_, app) = visible(state, p, project, name).await?;
    if may_read_logs(state, p, project, app.as_ref()).await? {
        Ok(())
    } else {
        Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            "you may not read this instance's logs (application.logs.read or instance.manage)",
        ))
    }
}

fn tail_kib(q: &TailQuery) -> Result<u32, ApiError> {
    let kib = q.tail_kib.unwrap_or(DEFAULT_TAIL_KIB);
    if (1..=MAX_TAIL_KIB).contains(&kib) {
        Ok(kib)
    } else {
        Err(super::deploy::bad_request(format!(
            "tailKib must be 1–{MAX_TAIL_KIB}"
        )))
    }
}

/// The instance's log files as Incus lists them (names only).
#[utoipa::path(get, path = "/api/v1/instances/{project}/{name}/logs", tag = "inventory", operation_id = "instance_logs",
    params(InstancePath),
    responses((status = 200, body = InstanceLogList), (status = 400, body = ErrorBody), (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 502, body = ErrorBody)))]
pub(super) async fn logs(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(InstancePath { project, name }): Path<InstancePath>,
) -> Result<Json<InstanceLogList>, ApiError> {
    log_access(&state, &p, &project, &name).await?;
    let files = state.provider.instance_logs(&project, &name).await?;
    Ok(Json(InstanceLogList {
        source: source(state.provider.mode()),
        files,
    }))
}

/// The end of one of the instance's log files (`tailKib`, default 64, at most 1024).
#[utoipa::path(get, path = "/api/v1/instances/{project}/{name}/logs/{file}", tag = "inventory", operation_id = "instance_log",
    params(LogPath, TailQuery),
    responses((status = 200, body = InstanceLogText), (status = 400, body = ErrorBody), (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 502, body = ErrorBody)))]
pub(super) async fn log_file(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(LogPath {
        project,
        name,
        file,
    }): Path<LogPath>,
    Query(q): Query<TailQuery>,
) -> Result<Json<InstanceLogText>, ApiError> {
    let kib = tail_kib(&q)?;
    if log_file_name(&file) != Some(file.as_str()) {
        return Err(super::deploy::bad_request(
            "invalid log file name; use a name from the list",
        ));
    }
    log_access(&state, &p, &project, &name).await?;
    // Only names Incus lists: nothing else is ever requested.
    if !state
        .provider
        .instance_logs(&project, &name)
        .await?
        .contains(&file)
    {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            format!("not found: log {file} of {project}/{name}"),
        ));
    }
    let log = state
        .provider
        .instance_log(&project, &name, &file, kib as usize * 1024)
        .await?;
    tracing::info!(instance = %format!("{project}/{name}"), %file, bytes = log.content.len(), "instance log read");
    Ok(Json(InstanceLogText {
        source: source(state.provider.mode()),
        file,
        tail_kib: kib,
        log,
    }))
}

/// The end of the instance's console log (`tailKib`, default 64, at most 1024).
#[utoipa::path(get, path = "/api/v1/instances/{project}/{name}/console-log", tag = "inventory", operation_id = "instance_console_log",
    params(InstancePath, TailQuery),
    responses((status = 200, body = InstanceLogText), (status = 400, body = ErrorBody), (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 502, body = ErrorBody)))]
pub(super) async fn console_log(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(InstancePath { project, name }): Path<InstancePath>,
    Query(q): Query<TailQuery>,
) -> Result<Json<InstanceLogText>, ApiError> {
    let kib = tail_kib(&q)?;
    log_access(&state, &p, &project, &name).await?;
    let log = state
        .provider
        .console_log(&project, &name, kib as usize * 1024)
        .await?;
    tracing::info!(instance = %format!("{project}/{name}"), bytes = log.content.len(), "instance console log read");
    Ok(Json(InstanceLogText {
        source: source(state.provider.mode()),
        file: "console".into(),
        tail_kib: kib,
        log,
    }))
}
