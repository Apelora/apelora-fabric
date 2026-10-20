// SPDX-License-Identifier: AGPL-3.0-only
//! Machines being enrolled (Phase 09). Holds who did what and when; the enrolment state itself
//! is derived from these facts and from Incus (`apelora_domain::enrolment`).

use sqlx::FromRow;
use time::OffsetDateTime;

use crate::{Pool, Result, StoreError};

const COLUMNS: &str = "name, provider, provider_ref, description, registered_by, registered_at, \
     approved_by, approved_at, approval_expires_at, admitted_by, admitted_at";

#[derive(Debug, Clone, PartialEq, FromRow)]
pub struct MachineRecord {
    pub name: String,
    pub provider: String,
    pub provider_ref: Option<String>,
    pub description: String,
    pub registered_by: String,
    pub registered_at: OffsetDateTime,
    pub approved_by: Option<String>,
    pub approved_at: Option<OffsetDateTime>,
    pub approval_expires_at: Option<String>,
    pub admitted_by: Option<String>,
    pub admitted_at: Option<OffsetDateTime>,
}

/// Registers a machine; `admitted` records an existing member as already admitted (adopted).
pub async fn register_machine(
    pool: &Pool,
    name: &str,
    provider: &str,
    description: &str,
    actor: &str,
    admitted: bool,
) -> Result<MachineRecord> {
    sqlx::query_as::<_, MachineRecord>(&format!(
        "INSERT INTO machines (name, provider, description, registered_by, admitted_by, admitted_at)
         VALUES ($1, $2, $3, $4, CASE WHEN $5 THEN $4 END, CASE WHEN $5 THEN now() END)
         ON CONFLICT (name) DO NOTHING
         RETURNING {COLUMNS}"
    ))
    .bind(name)
    .bind(provider)
    .bind(description)
    .bind(actor)
    .bind(admitted)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| StoreError::Conflict(format!("machine {name} is already registered")))
}

pub async fn machines(pool: &Pool) -> Result<Vec<MachineRecord>> {
    Ok(
        sqlx::query_as::<_, MachineRecord>(&format!(
            "SELECT {COLUMNS} FROM machines ORDER BY name"
        ))
        .fetch_all(pool)
        .await?,
    )
}

pub async fn machine(pool: &Pool, name: &str) -> Result<MachineRecord> {
    sqlx::query_as::<_, MachineRecord>(&format!("SELECT {COLUMNS} FROM machines WHERE name = $1"))
        .bind(name)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| StoreError::NotFound(format!("machine {name}")))
}

/// Records an approval (a join token was issued that expires at `expires_at`).
pub async fn approve_machine(pool: &Pool, name: &str, actor: &str, expires_at: &str) -> Result<()> {
    let q = sqlx::query(
        "UPDATE machines SET approved_by = $2, approved_at = now(), approval_expires_at = $3
         WHERE name = $1",
    )
    .bind(name)
    .bind(actor)
    .bind(expires_at);
    found(name, q.execute(pool).await?.rows_affected())
}

/// Forgets an approval whose token was revoked.
pub async fn withdraw_approval(pool: &Pool, name: &str) -> Result<()> {
    let q = sqlx::query(
        "UPDATE machines SET approved_by = NULL, approved_at = NULL, approval_expires_at = NULL
         WHERE name = $1",
    )
    .bind(name);
    found(name, q.execute(pool).await?.rows_affected())
}

pub async fn admit_machine(pool: &Pool, name: &str, actor: &str) -> Result<()> {
    let q =
        sqlx::query("UPDATE machines SET admitted_by = $2, admitted_at = now() WHERE name = $1")
            .bind(name)
            .bind(actor);
    found(name, q.execute(pool).await?.rows_affected())
}

pub async fn remove_machine(pool: &Pool, name: &str) -> Result<()> {
    let q = sqlx::query("DELETE FROM machines WHERE name = $1").bind(name);
    found(name, q.execute(pool).await?.rows_affected())
}

fn found(name: &str, rows: u64) -> Result<()> {
    if rows == 0 {
        return Err(StoreError::NotFound(format!("machine {name}")));
    }
    Ok(())
}
