// SPDX-License-Identifier: AGPL-3.0-only
//! The MAAS provisioner (ADR 0002, `docs/design/provisioner-maas.md`): what MAAS manages, next
//! to the machines Apelora has registered and the Incus members they became. Read-only; the API
//! key never leaves this process and nothing from a machine's power parameters is carried.

use std::sync::Arc;

use apelora_maas_adapter::{MaasClient, MaasError, Machine as MaasMachine, PowerState};
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;

use super::authz::{allow_platform, audit_scoped, begin, commit};
use super::cluster_admin::audit_store;
use super::{ApiError, AppState, ErrorBody, Principal, Source, source};

/// Where MAAS machines come from.
#[derive(Clone, Default)]
pub enum Maas {
    /// `APELORA_MAAS_URL` is not set: only manual machines can be registered.
    #[default]
    Off,
    Client(Arc<MaasClient>),
    /// Fixed answers (tests): the machines, or the error MAAS gives.
    #[cfg_attr(not(test), allow(dead_code))]
    Fixed(Arc<Result<Vec<MaasMachine>, MaasError>>),
}

impl std::fmt::Debug for Maas {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Maas::Off => "Maas::Off",
            Maas::Client(_) => "Maas::Client",
            Maas::Fixed(_) => "Maas::Fixed",
        })
    }
}

impl Maas {
    pub fn configured(&self) -> bool {
        !matches!(self, Maas::Off)
    }

    pub async fn machines(&self) -> Result<Vec<MaasMachine>, MaasError> {
        match self {
            Maas::Off => Err(MaasError::Invalid("MAAS is not configured".into())),
            Maas::Client(c) => c.machines().await,
            Maas::Fixed(r) => (**r).clone(),
        }
    }

    pub async fn machine(&self, system_id: &str) -> Result<MaasMachine, MaasError> {
        match self {
            Maas::Off => Err(MaasError::Invalid("MAAS is not configured".into())),
            Maas::Client(c) => c.machine(system_id).await,
            Maas::Fixed(r) => (**r)
                .clone()?
                .into_iter()
                .find(|m| m.system_id == system_id)
                .ok_or(MaasError::Refused {
                    status: 404,
                    what: format!("machines/{system_id}"),
                }),
        }
    }

    pub async fn commission(&self, system_id: &str) -> Result<(), MaasError> {
        match self {
            Maas::Off => Err(MaasError::Invalid("MAAS is not configured".into())),
            Maas::Client(c) => c.commission(system_id).await,
            Maas::Fixed(_) => {
                self.machine(system_id).await?;
                Ok(())
            }
        }
    }

    pub async fn deploy(
        &self,
        system_id: &str,
        distro_series: &str,
        user_data: &str,
    ) -> Result<(), MaasError> {
        match self {
            Maas::Off => Err(MaasError::Invalid("MAAS is not configured".into())),
            Maas::Client(c) => c.deploy(system_id, distro_series, user_data).await,
            Maas::Fixed(_) => {
                self.machine(system_id).await?;
                if user_data.trim().is_empty() || distro_series.is_empty() {
                    return Err(MaasError::Invalid(
                        "cloud-init or distro series is empty".into(),
                    ));
                }
                Ok(())
            }
        }
    }

    pub async fn release(&self, system_id: &str, erase: bool) -> Result<(), MaasError> {
        match self {
            Maas::Off => Err(MaasError::Invalid("MAAS is not configured".into())),
            Maas::Client(c) => c.release(system_id, erase).await,
            Maas::Fixed(_) => {
                self.machine(system_id).await?;
                Ok(())
            }
        }
    }
}

/// The MAAS client from the environment: `APELORA_MAAS_URL`, `APELORA_MAAS_API_KEY` (a secret,
/// read once) and optionally `APELORA_MAAS_CA`. A half-set configuration is an error, said at
/// start-up.
pub fn from_env(get: impl Fn(&str) -> Option<String>) -> Result<Maas, String> {
    let val = |k: &str| {
        get(k)
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    };
    let Some(url) = val("APELORA_MAAS_URL") else {
        if val("APELORA_MAAS_API_KEY").is_some() {
            return Err("APELORA_MAAS_API_KEY is set without APELORA_MAAS_URL".into());
        }
        return Ok(Maas::Off);
    };
    let key = val("APELORA_MAAS_API_KEY")
        .ok_or("APELORA_MAAS_URL is set without APELORA_MAAS_API_KEY")?;
    let key = apelora_maas_adapter::ApiKey::parse(&key).map_err(|e| e.to_string())?;
    let ca = val("APELORA_MAAS_CA").map(std::path::PathBuf::from);
    let client = MaasClient::new(&url, key, ca.as_deref()).map_err(|e| e.to_string())?;
    Ok(Maas::Client(Arc::new(client)))
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MaasMachineView {
    /// MAAS's identifier; give it when registering the machine.
    pub system_id: String,
    pub hostname: String,
    /// `New`, `Commissioning`, `Ready`, `Allocated`, `Deploying`, `Deployed`, `Failed …`.
    pub status: String,
    /// `on`, `off`, `unknown` or `error`, as MAAS last read it from the BMC.
    pub power_state: String,
    /// How MAAS switches it: `ipmi`, `redfish`, `manual`, `virsh`…
    pub power_type: String,
    pub architecture: String,
    pub cpu_count: u32,
    pub memory_mib: u64,
    pub os: String,
    pub addresses: Vec<String>,
    /// The server's hardware UUID (needed for the BMC fence mapping); absent until commissioned.
    pub hardware_uuid: Option<String>,
    pub zone: String,
    pub pool: String,
    /// The Apelora machine registered for it, when there is one.
    pub registered_as: Option<String>,
    /// The Incus member status of that machine, once it has joined.
    pub member_status: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MaasMachines {
    pub source: Source,
    /// `false` when `APELORA_MAAS_URL` is not set: nothing to list, and what to do about it.
    pub configured: bool,
    pub items: Vec<MaasMachineView>,
}

fn power(p: PowerState) -> &'static str {
    match p {
        PowerState::On => "on",
        PowerState::Off => "off",
        PowerState::Unknown => "unknown",
        PowerState::Error => "error",
    }
}

fn upstream(e: &MaasError) -> ApiError {
    ApiError::new(
        StatusCode::BAD_GATEWAY,
        "provisioner_unavailable",
        format!("{e}"),
    )
}

/// The machines MAAS manages, with the Apelora machine and Incus member each became. Needs
/// `cluster.view`. Read-only: MAAS is asked to list, nothing else.
#[utoipa::path(get, path = "/api/v1/provisioners/maas/machines", tag = "cluster", operation_id = "maas_machines",
    responses((status = 200, body = MaasMachines), (status = 401, body = ErrorBody), (status = 403, body = ErrorBody),
        (status = 502, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn machines(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Json<MaasMachines>, ApiError> {
    allow_platform(&p, "cluster.view")?;
    let src = source(state.provider.mode());
    if !state.maas.configured() {
        return Ok(Json(MaasMachines {
            source: src,
            configured: false,
            items: Vec::new(),
        }));
    }
    let listed = state.maas.machines().await.map_err(|e| upstream(&e))?;
    // What Apelora has registered for them, and how far those became members.
    let registered = match &state.auth {
        super::Auth::Tokens(pool) => apelora_persistence::machines(pool).await?,
        _ => Vec::new(),
    };
    let members = match state.admin.as_ref() {
        Some(a) => a.cluster_members().await.unwrap_or_default(),
        None => Vec::new(),
    };
    let items = listed
        .into_iter()
        .map(|m| {
            let rec = registered.iter().find(|r| {
                r.provider == "maas" && r.provider_ref.as_deref() == Some(m.system_id.as_str())
            });
            let member = rec.and_then(|r| members.iter().find(|x| x.name == r.name));
            MaasMachineView {
                registered_as: rec.map(|r| r.name.clone()),
                member_status: member.map(|x| x.status.clone()),
                power_state: power(m.power_state).to_owned(),
                system_id: m.system_id,
                hostname: m.hostname,
                status: m.status,
                power_type: m.power_type,
                architecture: m.architecture,
                cpu_count: m.cpu_count,
                memory_mib: m.memory_mib,
                os: m.os,
                addresses: m.addresses,
                hardware_uuid: m.hardware_uuid,
                zone: m.zone,
                pool: m.pool,
            }
        })
        .collect();
    Ok(Json(MaasMachines {
        source: src,
        configured: true,
        items,
    }))
}

/// Checks a machine to register as a MAAS machine: MAAS is configured, knows the system id, and
/// the id is not already registered under another name. Returns the id to record.
pub(super) async fn check_registration(
    state: &AppState,
    system_id: Option<&str>,
) -> Result<String, ApiError> {
    if !state.maas.configured() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "provisioner_not_configured",
            "no maas provisioner is configured (APELORA_MAAS_URL); register the machine as manual",
        ));
    }
    let id = system_id
        .map(str::trim)
        .filter(|i| !i.is_empty())
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "a machine from MAAS needs its systemId (see the MAAS list)",
            )
        })?;
    match state.maas.machine(id).await {
        Ok(_) => Ok(id.to_owned()),
        Err(MaasError::Invalid(m)) => {
            Err(ApiError::new(StatusCode::BAD_REQUEST, "invalid_request", m))
        }
        Err(MaasError::Refused { status: 404, .. }) => Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "unknown_machine",
            format!("MAAS does not know a machine {id}"),
        )),
        Err(e) => Err(upstream(&e)),
    }
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct MaasConfirm {
    /// The machine's hostname, typed again.
    pub confirm: String,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct MaasDeploy {
    pub confirm: String,
    /// MAAS's series name, such as `noble`.
    pub distro_series: String,
    /// Cloud-init. Sent to MAAS and not stored. A join token in it is not kept.
    pub user_data: String,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct MaasRelease {
    pub confirm: String,
    /// Wipe the installed system. Off unless the caller sets it. A secure wipe is not offered.
    #[serde(default)]
    pub erase_disks: bool,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct MaasAccepted {
    pub system_id: String,
    pub hostname: String,
    pub action: String,
}

fn maas_error(e: MaasError) -> ApiError {
    match e {
        MaasError::Invalid(m) => ApiError::new(StatusCode::BAD_REQUEST, "invalid_request", m),
        MaasError::Refused { status: 404, .. } => ApiError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "MAAS does not know that machine",
        ),
        other => upstream(&other),
    }
}

async fn known(state: &AppState, system_id: &str) -> Result<MaasMachine, ApiError> {
    if !state.maas.configured() {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "provisioner_not_configured",
            "MAAS is not configured (APELORA_MAAS_URL)",
        ));
    }
    state.maas.machine(system_id).await.map_err(maas_error)
}

/// Starts commissioning. Needs `hardware.commission` and the hostname typed again. The cloud-init
/// of a later deploy is not part of this. Audited; MAAS is asked first.
#[utoipa::path(post, path = "/api/v1/provisioners/maas/machines/{system_id}/commission", tag = "cluster",
    operation_id = "maas_commission",
    params(("system_id" = String, Path, description = "MAAS system id")),
    request_body = MaasConfirm,
    responses((status = 202, body = MaasAccepted), (status = 400, body = ErrorBody),
        (status = 401, body = ErrorBody), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody),
        (status = 502, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn commission(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(system_id): Path<String>,
    body: Result<Json<MaasConfirm>, JsonRejection>,
) -> Result<(StatusCode, Json<MaasAccepted>), ApiError> {
    allow_platform(&p, "hardware.commission")?;
    let Json(req) =
        body.map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, "invalid_request", e.body_text()))?;
    let machine = known(&state, &system_id).await?;
    if req.confirm != machine.hostname {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "confirmation_mismatch",
            format!("type the hostname ({}) to commission it", machine.hostname),
        ));
    }
    state
        .maas
        .commission(&system_id)
        .await
        .map_err(maas_error)?;
    let pool = audit_store(&state)?;
    let mut tx = begin(pool).await?;
    audit_scoped(
        &mut tx,
        &p,
        "maas.machine.commission",
        &system_id,
        "success",
        None,
        &json!({ "hostname": machine.hostname, "role": p.role }),
    )
    .await
    .map_err(|e| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "store_error",
            e.to_string(),
        )
    })?;
    commit(tx).await.map_err(|e| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "store_error",
            e.to_string(),
        )
    })?;
    Ok((
        StatusCode::ACCEPTED,
        Json(MaasAccepted {
            system_id,
            hostname: machine.hostname,
            action: "commission".into(),
        }),
    ))
}

/// Installs an operating system. Needs `hardware.deploy` and the hostname typed again.
/// `userData` is sent to MAAS and is not stored or written to the audit log. Audited.
#[utoipa::path(post, path = "/api/v1/provisioners/maas/machines/{system_id}/deploy", tag = "cluster",
    operation_id = "maas_deploy",
    params(("system_id" = String, Path, description = "MAAS system id")),
    request_body = MaasDeploy,
    responses((status = 202, body = MaasAccepted), (status = 400, body = ErrorBody),
        (status = 401, body = ErrorBody), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody),
        (status = 502, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn deploy(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(system_id): Path<String>,
    body: Result<Json<MaasDeploy>, JsonRejection>,
) -> Result<(StatusCode, Json<MaasAccepted>), ApiError> {
    allow_platform(&p, "hardware.deploy")?;
    let Json(req) =
        body.map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, "invalid_request", e.body_text()))?;
    let machine = known(&state, &system_id).await?;
    if req.confirm != machine.hostname {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "confirmation_mismatch",
            format!(
                "type the hostname ({}) to install an operating system",
                machine.hostname
            ),
        ));
    }
    let bytes = req.user_data.len();
    state
        .maas
        .deploy(&system_id, req.distro_series.trim(), &req.user_data)
        .await
        .map_err(maas_error)?;
    let pool = audit_store(&state)?;
    let mut tx = begin(pool).await?;
    audit_scoped(
        &mut tx,
        &p,
        "maas.machine.deploy",
        &system_id,
        "success",
        None,
        &json!({ "hostname": machine.hostname, "distroSeries": req.distro_series.trim(), "userDataBytes": bytes, "role": p.role }),
    )
    .await
    .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "store_error", e.to_string()))?;
    commit(tx).await.map_err(|e| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "store_error",
            e.to_string(),
        )
    })?;
    Ok((
        StatusCode::ACCEPTED,
        Json(MaasAccepted {
            system_id,
            hostname: machine.hostname,
            action: "deploy".into(),
        }),
    ))
}

/// Returns the machine to MAAS's pool. Needs `hardware.deploy` and the hostname typed again.
/// Disk erasure needs `hardware.delete` as well and is off unless asked. A secure wipe is not offered.
/// Audited; MAAS is asked first.
#[utoipa::path(post, path = "/api/v1/provisioners/maas/machines/{system_id}/release", tag = "cluster",
    operation_id = "maas_release",
    params(("system_id" = String, Path, description = "MAAS system id")),
    request_body = MaasRelease,
    responses((status = 202, body = MaasAccepted), (status = 400, body = ErrorBody),
        (status = 401, body = ErrorBody), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody),
        (status = 502, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn release(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(system_id): Path<String>,
    body: Result<Json<MaasRelease>, JsonRejection>,
) -> Result<(StatusCode, Json<MaasAccepted>), ApiError> {
    allow_platform(&p, "hardware.deploy")?;
    let Json(req) =
        body.map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, "invalid_request", e.body_text()))?;
    if req.erase_disks {
        allow_platform(&p, "hardware.delete")?;
    }
    let machine = known(&state, &system_id).await?;
    if req.confirm != machine.hostname {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "confirmation_mismatch",
            format!("type the hostname ({}) to release it", machine.hostname),
        ));
    }
    state
        .maas
        .release(&system_id, req.erase_disks)
        .await
        .map_err(maas_error)?;
    let pool = audit_store(&state)?;
    let mut tx = begin(pool).await?;
    audit_scoped(
        &mut tx,
        &p,
        "maas.machine.release",
        &system_id,
        "success",
        None,
        &json!({ "hostname": machine.hostname, "eraseDisks": req.erase_disks, "role": p.role }),
    )
    .await
    .map_err(|e| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "store_error",
            e.to_string(),
        )
    })?;
    commit(tx).await.map_err(|e| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "store_error",
            e.to_string(),
        )
    })?;
    Ok((
        StatusCode::ACCEPTED,
        Json(MaasAccepted {
            system_id,
            hostname: machine.hostname,
            action: "release".into(),
        }),
    ))
}
