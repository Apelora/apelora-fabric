// SPDX-License-Identifier: AGPL-3.0-only
//! An application's persistent volumes (docs/architecture/STORAGE.md). The record says which
//! volumes Apelora created; Incus says whether each still exists and who uses it. Removing an
//! appliance never deletes its data: deleting a volume is its own action, with the volume's
//! name typed again, refused while an instance uses it, and audited.

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;

use super::admin::rfc3339;
use super::authz::{allow, audit_scoped, guard};
use super::deploy::{bad_request, store};
use super::manage::load;
use super::{ApiError, AppState, ErrorBody, Principal, Source, source};

/// What Incus reports about a recorded volume now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum VolumeUse {
    /// An instance uses it.
    Attached,
    /// It exists and no instance uses it: its data is kept until someone deletes it.
    Retained,
    /// Incus has no such volume (deleted outside Apelora, or its member is unreachable).
    Missing,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct VolumeItem {
    pub appliance: String,
    pub instance: String,
    /// The design's name (`data`).
    pub name: String,
    /// The Incus volume (`<instance>-<name>`): what to type to delete it.
    pub incus_name: String,
    pub pool: String,
    /// Member of a member-local pool; empty on a standalone server.
    pub member: String,
    pub size_mib: i32,
    pub content: String,
    pub class: String,
    pub state: VolumeUse,
    /// Instances using it.
    pub used_by: Vec<String>,
    pub created_at: String,
}

#[derive(Serialize, ToSchema)]
pub struct ApplicationVolumes {
    pub source: Source,
    pub items: Vec<VolumeItem>,
}

#[derive(Deserialize, ToSchema)]
pub struct ConfirmVolume {
    /// The volume's Incus name again, typed by the person deleting it.
    pub confirm: String,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GrowVolume {
    /// The new size in MiB; at least the current one (volumes only grow).
    pub size_mib: u32,
}

/// The instance names in Incus's `used_by` paths (`/1.0/instances/<name>?project=…`).
fn instances_using(used_by: &[String]) -> Vec<String> {
    used_by
        .iter()
        .filter_map(|u| u.strip_prefix("/1.0/instances/"))
        .map(|u| u.split(['?', '/']).next().unwrap_or(u).to_owned())
        .collect()
}

fn member_of(r: &apelora_persistence::VolumeRecord) -> Option<&str> {
    Some(r.member.as_str()).filter(|m| !m.is_empty() && *m != "none")
}

/// The application's volumes, with what Incus says about each now. Needs `application.view`.
#[utoipa::path(get, path = "/api/v1/applications/{name}/volumes", tag = "applications", operation_id = "application_volumes",
    params(("name" = String, Path, description = "Application name")),
    responses((status = 200, body = ApplicationVolumes), (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 502, body = ErrorBody)))]
pub(super) async fn list(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(name): Path<String>,
) -> Result<Json<ApplicationVolumes>, ApiError> {
    load(&state, &p, &name).await?;
    let records = apelora_persistence::application_volumes(store(&state)?, &name).await?;
    let mut items = Vec::with_capacity(records.len());
    for r in records {
        let live = state
            .provider
            .volume(&r.project, &r.storage_pool, member_of(&r), &r.incus_name)
            .await?;
        let used_by = live
            .as_ref()
            .map(|v| instances_using(&v.used_by))
            .unwrap_or_default();
        items.push(VolumeItem {
            state: match &live {
                None => VolumeUse::Missing,
                Some(_) if used_by.is_empty() => VolumeUse::Retained,
                Some(_) => VolumeUse::Attached,
            },
            used_by,
            created_at: rfc3339(r.created_at),
            appliance: r.appliance,
            instance: r.instance,
            name: r.name,
            incus_name: r.incus_name,
            pool: r.storage_pool,
            member: r.member,
            size_mib: r.size_mib,
            content: r.content,
            class: r.class,
        });
    }
    Ok(Json(ApplicationVolumes {
        source: source(state.provider.mode()),
        items,
    }))
}

/// Grows one of the application's volumes, also while its instance runs. Volumes only grow:
/// a smaller size is refused. Needs `application.deploy`; audited.
#[utoipa::path(patch, path = "/api/v1/applications/{name}/volumes/{volume}", tag = "applications", operation_id = "grow_application_volume",
    params(("name" = String, Path, description = "Application name"),
        ("volume" = String, Path, description = "The volume's Incus name")),
    request_body = GrowVolume,
    responses((status = 204), (status = 400, body = ErrorBody), (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody),
        (status = 502, body = ErrorBody)))]
pub(super) async fn grow(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path((name, volume)): Path<(String, String)>,
    body: Result<Json<GrowVolume>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let (_, res) = load(&state, &p, &name).await?;
    allow(&p, "application.deploy", &res)?;
    let Json(req) = body.map_err(|e| bad_request(e.body_text()))?;
    let pool = store(&state)?;
    let record = apelora_persistence::application_volume(pool, &name, &volume)
        .await?
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "not_found",
                format!("not found: volume {volume} of application {name}"),
            )
        })?;
    let current = u32::try_from(record.size_mib).unwrap_or(0);
    apelora_domain::storage::check_resize(current, req.size_mib).map_err(bad_request)?;
    state
        .provider
        .grow_volume(
            &record.project,
            &record.storage_pool,
            member_of(&record),
            &record.incus_name,
            &name,
            req.size_mib,
        )
        .await?;
    apelora_persistence::set_volume_size(pool, record.id, req.size_mib).await?;
    audit_scoped(
        pool,
        &p,
        "application.volume.grow",
        &format!("{}/{}", record.project, record.incus_name),
        "success",
        Some(&res),
        &json!({ "application": name, "fromMib": current, "toMib": req.size_mib }),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Deletes one of the application's volumes and its snapshots, for good. The volume's Incus
/// name must be typed again; refused while an instance uses it. Needs `application.delete`
/// (and an approval where the policy asks for one); audited.
#[utoipa::path(delete, path = "/api/v1/applications/{name}/volumes/{volume}", tag = "applications", operation_id = "delete_application_volume",
    params(("name" = String, Path, description = "Application name"),
        ("volume" = String, Path, description = "The volume's Incus name")),
    request_body = ConfirmVolume,
    responses((status = 204), (status = 400, body = ErrorBody), (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody),
        (status = 502, body = ErrorBody)))]
pub(super) async fn delete(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    headers: HeaderMap,
    Path((name, volume)): Path<(String, String)>,
    body: Result<Json<ConfirmVolume>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let (_, res) = load(&state, &p, &name).await?;
    allow(&p, "application.delete", &res)?;
    let Json(req) = body.map_err(|e| bad_request(e.body_text()))?;
    if req.confirm != volume {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "confirmation_mismatch",
            format!("type the volume's name ({volume}) to confirm"),
        ));
    }
    let pool = store(&state)?;
    let record = apelora_persistence::application_volume(pool, &name, &volume)
        .await?
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "not_found",
                format!("not found: volume {volume} of application {name}"),
            )
        })?;
    let target = format!("{}/{}", record.project, record.incus_name);
    if let Some(live) = state
        .provider
        .volume(
            &record.project,
            &record.storage_pool,
            member_of(&record),
            &record.incus_name,
        )
        .await?
    {
        let users = instances_using(&live.used_by);
        if !users.is_empty() {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "volume_in_use",
                format!(
                    "nothing was deleted: {} uses the volume; remove the appliance from the design first",
                    users.join(", ")
                ),
            ));
        }
    }
    let approval = guard(&state, &p, &headers, "application.delete", &target).await?;
    let existed = state
        .provider
        .delete_volume(
            &record.project,
            &record.storage_pool,
            member_of(&record),
            &record.incus_name,
            &name,
        )
        .await?;
    apelora_persistence::forget_volume(pool, record.id).await?;
    audit_scoped(
        pool,
        &p,
        "application.volume.delete",
        &target,
        "success",
        Some(&res),
        &json!({ "application": name, "appliance": record.appliance, "pool": record.storage_pool,
                 "member": record.member, "sizeMib": record.size_mib, "existed": existed, "approval": approval }),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    #[test]
    fn instance_names_are_read_from_incus_paths() {
        assert_eq!(
            super::instances_using(&[
                "/1.0/instances/shop-db-01?project=apelora".into(),
                "/1.0/instances/plain".into(),
                "/1.0/profiles/default".into(),
            ]),
            vec!["shop-db-01".to_owned(), "plain".to_owned()]
        );
    }
}
