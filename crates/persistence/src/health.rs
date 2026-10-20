// SPDX-License-Identifier: AGPL-3.0-only
//! Instance health decided by the supervisor (Phase 08); read by the status API.

use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{Pool, Result};

#[derive(Debug, Clone, PartialEq, FromRow)]
pub struct HealthRecord {
    pub project: String,
    pub instance: String,
    pub appliance: String,
    pub health: String,
    pub since: OffsetDateTime,
    pub checked_at: OffsetDateTime,
    pub detail: String,
    pub restarted_at: Option<OffsetDateTime>,
}

/// One instance's health as decided now.
pub struct NewHealth<'a> {
    pub application_id: Uuid,
    pub project: &'a str,
    pub instance: &'a str,
    pub appliance: &'a str,
    pub health: &'a str,
    pub since: OffsetDateTime,
    pub detail: &'a str,
    pub restarted_at: Option<OffsetDateTime>,
}

pub async fn record_health(pool: &Pool, h: &NewHealth<'_>) -> Result<()> {
    sqlx::query(
        "INSERT INTO instance_health
           (project, instance, application_id, appliance, health, since, detail, restarted_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         ON CONFLICT (project, instance) DO UPDATE SET
           application_id = EXCLUDED.application_id, appliance = EXCLUDED.appliance,
           health = EXCLUDED.health, since = EXCLUDED.since, checked_at = now(),
           detail = EXCLUDED.detail,
           restarted_at = coalesce(EXCLUDED.restarted_at, instance_health.restarted_at)",
    )
    .bind(h.project)
    .bind(h.instance)
    .bind(h.application_id)
    .bind(h.appliance)
    .bind(h.health)
    .bind(h.since)
    .bind(h.detail)
    .bind(h.restarted_at)
    .execute(pool)
    .await?;
    Ok(())
}

/// The application's instances with a recorded health, in every project.
pub async fn application_health(pool: &Pool, application: &str) -> Result<Vec<HealthRecord>> {
    Ok(sqlx::query_as::<_, HealthRecord>(
        "SELECT h.project, h.instance, h.appliance, h.health, h.since, h.checked_at, h.detail,
                h.restarted_at
         FROM instance_health h JOIN applications a ON a.id = h.application_id
         WHERE a.name = $1 ORDER BY h.project, h.instance",
    )
    .bind(application)
    .fetch_all(pool)
    .await?)
}

/// Forgets the application's instances that are no longer checked.
pub async fn forget_health(
    pool: &Pool,
    application_id: Uuid,
    project: &str,
    keep: &[String],
) -> Result<u64> {
    Ok(sqlx::query(
        "DELETE FROM instance_health
         WHERE application_id = $1 AND project = $2 AND NOT (instance = ANY($3))",
    )
    .bind(application_id)
    .bind(project)
    .bind(keep)
    .execute(pool)
    .await?
    .rows_affected())
}
