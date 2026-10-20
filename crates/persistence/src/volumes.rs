// SPDX-License-Identifier: AGPL-3.0-only
//! Volumes Apelora created (storage step 2). Written by the worker, read and deleted by the API.

use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{Pool, Result};

#[derive(Debug, Clone, PartialEq, FromRow)]
pub struct VolumeRecord {
    pub id: Uuid,
    pub project: String,
    pub module: String,
    pub instance: String,
    pub name: String,
    pub incus_name: String,
    pub storage_pool: String,
    pub member: String,
    pub size_mib: i32,
    pub content: String,
    pub class: String,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

/// A volume that now exists in Incus.
pub struct NewVolume<'a> {
    pub application: &'a str,
    pub project: &'a str,
    pub module: &'a str,
    pub instance: &'a str,
    pub name: &'a str,
    pub incus_name: &'a str,
    pub storage_pool: &'a str,
    pub member: &'a str,
    pub size_mib: u32,
    pub content: &'a str,
    pub class: &'a str,
}

/// Records a volume (again): repeated calls keep one row. `false` when the application is not
/// known to the database (an instance created outside a deployment).
pub async fn record_volume(pool: &Pool, v: &NewVolume<'_>) -> Result<bool> {
    let size = i32::try_from(v.size_mib).unwrap_or(i32::MAX);
    Ok(sqlx::query(
        "INSERT INTO volumes (id, application_id, project, module, instance, name, incus_name,
                              storage_pool, member, size_mib, content, class)
         SELECT $1, a.id, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12
         FROM applications a WHERE a.name = $2
         ON CONFLICT (project, storage_pool, incus_name) DO UPDATE SET
           member = EXCLUDED.member, size_mib = EXCLUDED.size_mib, updated_at = now()",
    )
    .bind(Uuid::new_v4())
    .bind(v.application)
    .bind(v.project)
    .bind(v.module)
    .bind(v.instance)
    .bind(v.name)
    .bind(v.incus_name)
    .bind(v.storage_pool)
    .bind(v.member)
    .bind(size)
    .bind(v.content)
    .bind(v.class)
    .execute(pool)
    .await?
    .rows_affected()
        > 0)
}

const COLUMNS: &str = "v.id, v.project, v.module, v.instance, v.name, v.incus_name,
    v.storage_pool, v.member, v.size_mib, v.content, v.class, v.created_at, v.updated_at";

/// The application's volumes, by module and name.
pub async fn application_volumes(pool: &Pool, application: &str) -> Result<Vec<VolumeRecord>> {
    Ok(sqlx::query_as::<_, VolumeRecord>(&format!(
        "SELECT {COLUMNS} FROM volumes v JOIN applications a ON a.id = v.application_id
         WHERE a.name = $1 ORDER BY v.module, v.instance, v.name"
    ))
    .bind(application)
    .fetch_all(pool)
    .await?)
}

/// Every recorded volume, of every application.
pub async fn all_volumes(pool: &Pool) -> Result<Vec<VolumeRecord>> {
    Ok(sqlx::query_as::<_, VolumeRecord>(&format!(
        "SELECT {COLUMNS} FROM volumes v ORDER BY v.member, v.project, v.incus_name"
    ))
    .fetch_all(pool)
    .await?)
}

/// One of the application's volumes, by its Incus name.
pub async fn application_volume(
    pool: &Pool,
    application: &str,
    incus_name: &str,
) -> Result<Option<VolumeRecord>> {
    Ok(sqlx::query_as::<_, VolumeRecord>(&format!(
        "SELECT {COLUMNS} FROM volumes v JOIN applications a ON a.id = v.application_id
         WHERE a.name = $1 AND v.incus_name = $2"
    ))
    .bind(application)
    .bind(incus_name)
    .fetch_optional(pool)
    .await?)
}

/// Records a volume's new size after it grew.
pub async fn set_volume_size(pool: &Pool, id: Uuid, size_mib: u32) -> Result<()> {
    sqlx::query("UPDATE volumes SET size_mib = $2, updated_at = now() WHERE id = $1")
        .bind(id)
        .bind(i32::try_from(size_mib).unwrap_or(i32::MAX))
        .execute(pool)
        .await?;
    Ok(())
}

/// Forgets a volume once it is deleted in Incus.
pub async fn forget_volume(pool: &Pool, id: Uuid) -> Result<bool> {
    Ok(sqlx::query("DELETE FROM volumes WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected()
        > 0)
}
