// SPDX-License-Identifier: AGPL-3.0-only
//! Registered sectors: other Incus clusters, by name, site, address and the name of their
//! credentials (migration 0035). Nothing secret is stored.

use apelora_domain::sites::SectorRegistration;
use sqlx::FromRow;
use time::OffsetDateTime;

use crate::{Pool, Result, StoreError};

#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct SectorRow {
    pub name: String,
    pub site: String,
    pub url: String,
    pub credentials: String,
    pub added_by: String,
    pub added_at: OffsetDateTime,
}

impl SectorRow {
    pub fn registration(&self) -> SectorRegistration {
        SectorRegistration {
            name: self.name.clone(),
            site: self.site.clone(),
            url: self.url.clone(),
            credentials: self.credentials.clone(),
        }
    }
}

pub async fn list(pool: &Pool) -> Result<Vec<SectorRow>> {
    Ok(sqlx::query_as::<_, SectorRow>(
        "SELECT name, site, url, credentials, added_by, added_at FROM sectors ORDER BY site, name",
    )
    .fetch_all(pool)
    .await?)
}

/// Adds a sector; `Conflict` when the name is taken.
pub async fn add(
    conn: &mut sqlx::PgConnection,
    r: &SectorRegistration,
    by: &str,
) -> Result<SectorRow> {
    sqlx::query_as::<_, SectorRow>(
        "INSERT INTO sectors (name, site, url, credentials, added_by) VALUES ($1, $2, $3, $4, $5)
         RETURNING name, site, url, credentials, added_by, added_at",
    )
    .bind(&r.name)
    .bind(&r.site)
    .bind(&r.url)
    .bind(&r.credentials)
    .bind(by)
    .fetch_one(conn)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(d) if d.is_unique_violation() => {
            StoreError::Conflict(format!("sector {} is already registered", r.name))
        }
        _ => StoreError::Database(e),
    })
}

/// Forgets a sector; returns whether it was there.
pub async fn remove(conn: &mut sqlx::PgConnection, name: &str) -> Result<bool> {
    Ok(sqlx::query("DELETE FROM sectors WHERE name = $1")
        .bind(name)
        .execute(conn)
        .await?
        .rows_affected()
        == 1)
}
