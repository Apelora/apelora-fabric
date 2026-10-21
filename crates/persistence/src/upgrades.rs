// SPDX-License-Identifier: AGPL-3.0-only
//! The rolling-upgrade run (migration 0031). One running row; its steps are the members in
//! order and the phase each has reached.

use apelora_domain::upgrade::UpgradeStep;
use serde_json::Value;
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{Pool, Result};

#[derive(Debug, Clone, FromRow)]
pub struct UpgradeRun {
    pub id: Uuid,
    pub status: String,
    pub started_by: String,
    pub started_at: OffsetDateTime,
    pub steps: Value,
}

impl UpgradeRun {
    pub fn steps(&self) -> Vec<UpgradeStep> {
        serde_json::from_value(self.steps.clone()).unwrap_or_default()
    }
}

/// The run that is still going, if there is one.
pub async fn running(pool: &Pool) -> Result<Option<UpgradeRun>> {
    Ok(sqlx::query_as::<_, UpgradeRun>(
        "SELECT id, status, started_by, started_at, steps FROM cluster_upgrades
         WHERE status = 'running'",
    )
    .fetch_optional(pool)
    .await?)
}

pub async fn start(
    conn: &mut sqlx::PgConnection,
    by: &str,
    steps: &[UpgradeStep],
) -> Result<UpgradeRun> {
    let steps =
        serde_json::to_value(steps).map_err(|e| crate::StoreError::Conflict(e.to_string()))?;
    Ok(sqlx::query_as::<_, UpgradeRun>(
        "INSERT INTO cluster_upgrades (id, status, started_by, steps)
         VALUES ($1, 'running', $2, $3)
         RETURNING id, status, started_by, started_at, steps",
    )
    .bind(Uuid::new_v4())
    .bind(by)
    .bind(steps)
    .fetch_one(&mut *conn)
    .await?)
}

pub async fn save_steps(
    conn: &mut sqlx::PgConnection,
    id: Uuid,
    steps: &[UpgradeStep],
    done: bool,
) -> Result<()> {
    let steps =
        serde_json::to_value(steps).map_err(|e| crate::StoreError::Conflict(e.to_string()))?;
    sqlx::query(
        "UPDATE cluster_upgrades
         SET steps = $2,
             status = CASE WHEN $3 THEN 'done' ELSE status END,
             finished_at = CASE WHEN $3 THEN now() ELSE finished_at END
         WHERE id = $1 AND status = 'running'",
    )
    .bind(id)
    .bind(steps)
    .bind(done)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

pub async fn abandon(conn: &mut sqlx::PgConnection, id: Uuid) -> Result<()> {
    sqlx::query(
        "UPDATE cluster_upgrades SET status = 'abandoned', finished_at = now()
         WHERE id = $1 AND status = 'running'",
    )
    .bind(id)
    .execute(&mut *conn)
    .await?;
    Ok(())
}
