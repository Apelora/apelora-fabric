// SPDX-License-Identifier: AGPL-3.0-only
//! Sectors (ADR 0008, slice 1): the Incus cluster this control plane runs against, and the
//! other clusters it is told about. Each registered sector is reached at its own address with
//! its own identity, whose files the `APELORA_SECTOR_<credentials>_*` variables name; only the
//! name of the credentials is stored. This slice reads each sector's state and counts its sites
//! for the members report; applications still deploy only to the home sector.

use std::time::Duration;

use apelora_domain::sites::{SectorRegistration, check_registration, credential_vars};
use apelora_incus_adapter::Provider;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;

use super::authz::{allow_platform, audit_scoped, begin, commit};
use super::{ApiError, AppState, ErrorBody, Principal, Source, source};

/// How long one sector's Incus may take to answer before it counts as unreachable.
const PROBE_BUDGET: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SectorState {
    /// Its Incus answered just now.
    Connected,
    /// Its Incus did not answer, or refused the identity.
    Unreachable,
    /// The control plane lacks the identity's files (`APELORA_SECTOR_<credentials>_*`).
    NotConfigured,
}

/// One sector as the Servers area shows it.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SectorInfo {
    pub name: String,
    pub site: String,
    /// The sector this control plane runs against (`APELORA_SITE`, `APELORA_SECTOR`).
    pub home: bool,
    /// Its Incus API; `null` for the home sector (`APELORA_INCUS_URL`, a setting).
    pub url: Option<String>,
    pub state: SectorState,
    /// Members of its cluster (1 for a standalone server); `null` when it did not answer.
    pub members: Option<u32>,
    pub incus_version: Option<String>,
    /// Why it is not connected, in a sentence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// For a registered sector: the variables that must name its identity's files.
    pub credential_vars: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added_at: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SectorList {
    pub source: Source,
    /// The home sector first, then the registered ones by site and name.
    pub items: Vec<SectorInfo>,
    /// Applications deploy only to the home sector in this version.
    pub note: String,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NewSector {
    pub name: String,
    pub site: String,
    /// `https://host:port` of its Incus API.
    pub url: String,
    /// Names the variables `APELORA_SECTOR_<credentials>_CLIENT_CERT`, `_CLIENT_KEY` and
    /// `_SERVER_CERT` on the control plane. Never a file or a secret.
    pub credentials: String,
}

#[derive(Deserialize, ToSchema)]
pub struct RemoveSector {
    /// The sector's name, typed again.
    pub confirm: String,
}

fn warning_for(members: Option<u32>) -> Option<String> {
    members
        .filter(|m| *m >= apelora_domain::sites::MEMBER_WARN)
        .map(|m| {
            format!(
                "{m} members: a sector is meant to stay at {} or fewer (ADR 0008); add a sector instead of growing this one",
                apelora_domain::sites::MEMBER_WARN
            )
        })
}

/// Asks one Incus who it is and how many members it has, within [`PROBE_BUDGET`].
async fn probe(provider: &Provider) -> (SectorState, Option<u32>, Option<String>, Option<String>) {
    let read = async {
        let info = provider.server_info().await?;
        let members = if info.clustered {
            u32::try_from(provider.cluster_members().await?.len()).unwrap_or(u32::MAX)
        } else {
            1
        };
        Ok::<_, apelora_incus_adapter::AdapterError>((members, info.server_version))
    };
    match tokio::time::timeout(PROBE_BUDGET, read).await {
        Ok(Ok((members, version))) => (SectorState::Connected, Some(members), version, None),
        Ok(Err(e)) => (SectorState::Unreachable, None, None, Some(e.to_string())),
        Err(_) => (
            SectorState::Unreachable,
            None,
            None,
            Some(format!("no answer within {} s", PROBE_BUDGET.as_secs())),
        ),
    }
}

fn home_place() -> Result<apelora_metrics::Place, ApiError> {
    apelora_metrics::Place::from_env(|key| std::env::var(key).ok()).map_err(|message| {
        ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "invalid_place", message)
    })
}

/// Every sector with its state, read now: the home sector and each registered one, probed
/// concurrently. Without a database only the home sector is known.
pub(super) async fn observed(state: &AppState) -> Result<Vec<SectorInfo>, ApiError> {
    let place = home_place()?;
    let (home_state, home_members, home_version, home_detail) = probe(&state.provider).await;
    let mut items = vec![SectorInfo {
        name: place.sector.clone(),
        site: place.site.clone(),
        home: true,
        url: None,
        state: home_state,
        members: home_members,
        incus_version: home_version,
        detail: home_detail,
        credential_vars: Vec::new(),
        warning: warning_for(home_members),
        added_by: None,
        added_at: None,
    }];
    items.extend(registered(state).await?);
    Ok(items)
}

/// The registered sectors with their state, probed concurrently; empty without a database.
pub(super) async fn registered(state: &AppState) -> Result<Vec<SectorInfo>, ApiError> {
    let super::Auth::Tokens(pool) = &state.auth else {
        return Ok(Vec::new());
    };
    let mut items = Vec::new();
    let rows = apelora_persistence::sectors::list(pool).await?;
    let mut set = tokio::task::JoinSet::new();
    for (i, row) in rows.iter().enumerate() {
        let (url, credentials) = (row.url.clone(), row.credentials.clone());
        let deploy = state.provider.clone();
        set.spawn(async move {
            let built = apelora_incus_adapter::config::sector_provider(
                |k| std::env::var(k).ok(),
                &url,
                &credentials,
                &deploy,
            );
            let r = match built {
                Ok(p) => probe(&p).await,
                Err(e) => (SectorState::NotConfigured, None, None, Some(e)),
            };
            (i, r)
        });
    }
    let mut probed = vec![
        (
            SectorState::Unreachable,
            None,
            None,
            Some("not read".to_owned())
        );
        rows.len()
    ];
    while let Some(done) = set.join_next().await {
        if let Ok((i, r)) = done {
            probed[i] = r;
        }
    }
    for (row, (st, members, version, detail)) in rows.into_iter().zip(probed) {
        items.push(SectorInfo {
            credential_vars: credential_vars(&row.credentials).to_vec(),
            name: row.name,
            site: row.site,
            home: false,
            url: Some(row.url),
            state: st,
            members,
            incus_version: version,
            detail,
            warning: warning_for(members),
            added_by: Some(row.added_by),
            added_at: Some(row.added_at.to_string()),
        });
    }
    Ok(items)
}

const NOTE: &str = "Applications deploy to the home sector only. Registered sectors are read for their state and count towards the sites a second deployment could use; deploying to them is a later step.";

/// The sectors this control plane knows and whether each answers. Needs `cluster.view`.
#[utoipa::path(get, path = "/api/v1/sectors", tag = "cluster", operation_id = "sectors",
    responses((status = 200, body = SectorList), (status = 401, body = ErrorBody), (status = 403, body = ErrorBody)))]
pub(super) async fn list(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Json<SectorList>, ApiError> {
    allow_platform(&p, "cluster.view")?;
    Ok(Json(SectorList {
        source: source(state.provider.mode()),
        items: observed(&state).await?,
        note: NOTE.into(),
    }))
}

/// Registers another Incus cluster as a sector. Stores its name, site, address and the name of
/// its credentials; the identity's files stay on the control plane. Needs `cluster.manage`.
#[utoipa::path(post, path = "/api/v1/sectors", tag = "cluster", operation_id = "add_sector",
    request_body = NewSector,
    responses((status = 201, body = SectorInfo), (status = 400, body = ErrorBody), (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody), (status = 409, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn add(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    body: Result<Json<NewSector>, JsonRejection>,
) -> Result<(StatusCode, Json<SectorInfo>), ApiError> {
    allow_platform(&p, "cluster.manage")?;
    let Json(req) =
        body.map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, "invalid_request", e.body_text()))?;
    let r = SectorRegistration {
        name: req.name.trim().to_owned(),
        site: req.site.trim().to_owned(),
        url: req.url.trim().trim_end_matches('/').to_owned(),
        credentials: req.credentials.trim().to_owned(),
    };
    if let Some(why) = check_registration(&r) {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_sector",
            why,
        ));
    }
    let place = home_place()?;
    if r.name == place.sector {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "sector_exists",
            format!("{} is the sector this control plane runs against", r.name),
        ));
    }
    let super::Auth::Tokens(pool) = &state.auth else {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "no_database",
            "registering a sector needs the database",
        ));
    };
    let mut tx = begin(pool).await?;
    let row = apelora_persistence::sectors::add(&mut tx, &r, &p.actor_ref())
        .await
        .map_err(|e| match e {
            apelora_persistence::StoreError::Conflict(m) => {
                ApiError::new(StatusCode::CONFLICT, "sector_exists", m)
            }
            other => other.into(),
        })?;
    audit_scoped(
        &mut tx,
        &p,
        "sector.add",
        &format!("sector:{}", r.name),
        "success",
        None,
        &json!({ "site": r.site, "url": r.url, "credentials": r.credentials, "role": p.role }),
    )
    .await?;
    commit(tx).await?;
    let info = observed(&state)
        .await?
        .into_iter()
        .find(|s| !s.home && s.name == row.name)
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "the sector was not stored",
            )
        })?;
    Ok((StatusCode::CREATED, Json(info)))
}

/// Forgets a registered sector. Nothing changes in its Incus. Needs `cluster.manage`.
#[utoipa::path(delete, path = "/api/v1/sectors/{name}", tag = "cluster", operation_id = "remove_sector",
    params(("name" = String, Path, description = "Sector name")),
    request_body = RemoveSector,
    responses((status = 204), (status = 400, body = ErrorBody), (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn remove(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(name): Path<String>,
    body: Result<Json<RemoveSector>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    allow_platform(&p, "cluster.manage")?;
    let Json(req) =
        body.map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, "invalid_request", e.body_text()))?;
    if req.confirm != name {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "confirmation_mismatch",
            "type the sector's name to forget it",
        ));
    }
    let super::Auth::Tokens(pool) = &state.auth else {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "no_database",
            "sectors are kept in the database",
        ));
    };
    let mut tx = begin(pool).await?;
    if !apelora_persistence::sectors::remove(&mut tx, &name).await? {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            format!("not found: sector {name}"),
        ));
    }
    audit_scoped(
        &mut tx,
        &p,
        "sector.remove",
        &format!("sector:{name}"),
        "success",
        None,
        &json!({ "role": p.role }),
    )
    .await?;
    commit(tx).await?;
    Ok(StatusCode::NO_CONTENT)
}
