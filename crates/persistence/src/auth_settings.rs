// SPDX-License-Identifier: AGPL-3.0-only
//! Identity providers and platform security settings configured in the Fabric Console.

use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{Pool, Result};

#[derive(Debug, Clone, PartialEq, FromRow)]
pub struct ProviderRecord {
    pub id: Uuid,
    pub name: String,
    pub issuer: String,
    pub audiences: Vec<String>,
    pub client_id: Option<String>,
    pub scopes: Vec<String>,
    pub groups_claim: String,
    pub username_claim: String,
    pub mfa_amr: Vec<String>,
    pub mfa_acr: Vec<String>,
    pub authorized_parties: Vec<String>,
    pub enabled: bool,
    pub updated_at: OffsetDateTime,
    pub updated_by: String,
}

/// The editable fields of a provider.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderFields {
    pub name: String,
    pub issuer: String,
    pub audiences: Vec<String>,
    pub client_id: Option<String>,
    pub scopes: Vec<String>,
    pub groups_claim: String,
    pub username_claim: String,
    pub mfa_amr: Vec<String>,
    pub mfa_acr: Vec<String>,
    pub authorized_parties: Vec<String>,
    pub enabled: bool,
}

const COLUMNS: &str =
    "id, name, issuer, audiences, client_id, scopes, groups_claim, username_claim,
    mfa_amr, mfa_acr, authorized_parties, enabled, updated_at, updated_by";

pub async fn list_providers(pool: &Pool) -> Result<Vec<ProviderRecord>> {
    Ok(sqlx::query_as::<_, ProviderRecord>(&format!(
        "SELECT {COLUMNS} FROM identity_providers ORDER BY name"
    ))
    .fetch_all(pool)
    .await?)
}

pub async fn get_provider(
    db: impl sqlx::PgExecutor<'_>,
    id: Uuid,
) -> Result<Option<ProviderRecord>> {
    Ok(sqlx::query_as::<_, ProviderRecord>(&format!(
        "SELECT {COLUMNS} FROM identity_providers WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(db)
    .await?)
}

/// Creates (`id` none) or replaces a provider; returns its id.
pub async fn save_provider(
    db: impl sqlx::PgExecutor<'_>,
    id: Option<Uuid>,
    f: &ProviderFields,
    actor: &str,
) -> Result<Uuid> {
    let id = id.unwrap_or_else(Uuid::new_v4);
    sqlx::query(
        "INSERT INTO identity_providers (id, name, issuer, audiences, client_id, scopes, groups_claim,
             username_claim, mfa_amr, mfa_acr, authorized_parties, enabled, created_by, updated_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $13)
         ON CONFLICT (id) DO UPDATE SET
           name = EXCLUDED.name, issuer = EXCLUDED.issuer, audiences = EXCLUDED.audiences,
           client_id = EXCLUDED.client_id, scopes = EXCLUDED.scopes,
           groups_claim = EXCLUDED.groups_claim, username_claim = EXCLUDED.username_claim,
           mfa_amr = EXCLUDED.mfa_amr, mfa_acr = EXCLUDED.mfa_acr,
           authorized_parties = EXCLUDED.authorized_parties, enabled = EXCLUDED.enabled,
           updated_at = now(), updated_by = EXCLUDED.updated_by",
    )
    .bind(id)
    .bind(&f.name)
    .bind(&f.issuer)
    .bind(&f.audiences)
    .bind(&f.client_id)
    .bind(&f.scopes)
    .bind(&f.groups_claim)
    .bind(&f.username_claim)
    .bind(&f.mfa_amr)
    .bind(&f.mfa_acr)
    .bind(&f.authorized_parties)
    .bind(f.enabled)
    .bind(actor)
    .execute(db)
    .await?;
    Ok(id)
}

pub async fn delete_provider(db: impl sqlx::PgExecutor<'_>, id: Uuid) -> Result<bool> {
    Ok(sqlx::query("DELETE FROM identity_providers WHERE id = $1")
        .bind(id)
        .execute(db)
        .await?
        .rows_affected()
        > 0)
}

/// A platform setting's value, if one was saved.
pub async fn setting(pool: &Pool, key: &str) -> Result<Option<serde_json::Value>> {
    Ok(sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT value FROM platform_settings WHERE key = $1",
    )
    .bind(key)
    .fetch_optional(pool)
    .await?)
}

pub async fn set_setting(
    db: impl sqlx::PgExecutor<'_>,
    key: &str,
    value: &serde_json::Value,
    actor: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO platform_settings (key, value, updated_by) VALUES ($1, $2, $3)
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = now(), updated_by = EXCLUDED.updated_by",
    )
    .bind(key)
    .bind(value)
    .bind(actor)
    .execute(db)
    .await?;
    Ok(())
}
