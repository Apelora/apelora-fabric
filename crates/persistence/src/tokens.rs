// SPDX-License-Identifier: AGPL-3.0-only
//! API tokens: 256 random bits shown once; only their SHA-256 hash is stored.

use sha2::{Digest, Sha256};
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{Pool, Result, StoreError};

const PREFIX: &str = "apl_";

/// A token as stored (never contains the token itself).
#[derive(Debug, Clone, PartialEq, FromRow)]
pub struct TokenRecord {
    pub id: Uuid,
    pub name: String,
    pub role: String,
    pub created_at: OffsetDateTime,
    pub expires_at: Option<OffsetDateTime>,
    pub revoked_at: Option<OffsetDateTime>,
    pub last_used_at: Option<OffsetDateTime>,
}

fn hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

fn new_secret() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|e| StoreError::Conflict(format!("no secure randomness available: {e}")))?;
    Ok(format!(
        "{PREFIX}{}",
        bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
    ))
}

/// Creates a token and returns it with its record. The plaintext exists only in the return value.
pub async fn create_token(
    pool: &Pool,
    name: &str,
    role: &str,
    expires_at: Option<OffsetDateTime>,
) -> Result<(String, TokenRecord)> {
    let secret = new_secret()?;
    let rec = sqlx::query_as::<_, TokenRecord>(
        "INSERT INTO api_tokens (id, name, token_hash, role, expires_at) VALUES ($1, $2, $3, $4, $5)
         RETURNING id, name, role, created_at, expires_at, revoked_at, last_used_at",
    )
    .bind(Uuid::new_v4())
    .bind(name)
    .bind(hash(&secret))
    .bind(role)
    .bind(expires_at)
    .fetch_one(pool)
    .await?;
    Ok((secret, rec))
}

/// The record for a presented token if it is known, not revoked and not expired. Also records
/// when it was last used.
pub async fn authenticate(pool: &Pool, presented: &str) -> Result<Option<TokenRecord>> {
    if !presented.starts_with(PREFIX) || presented.len() != PREFIX.len() + 64 {
        return Ok(None);
    }
    Ok(sqlx::query_as::<_, TokenRecord>(
        "UPDATE api_tokens SET last_used_at = now()
         WHERE token_hash = $1 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at > now())
         RETURNING id, name, role, created_at, expires_at, revoked_at, last_used_at",
    )
    .bind(hash(presented))
    .fetch_optional(pool)
    .await?)
}

pub async fn revoke_token(pool: &Pool, id: Uuid) -> Result<()> {
    let n = sqlx::query(
        "UPDATE api_tokens SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL",
    )
    .bind(id)
    .execute(pool)
    .await?
    .rows_affected();
    if n == 1 {
        Ok(())
    } else {
        Err(StoreError::NotFound(format!("active token {id}")))
    }
}

pub async fn list_tokens(pool: &Pool) -> Result<Vec<TokenRecord>> {
    Ok(sqlx::query_as::<_, TokenRecord>("SELECT id, name, role, created_at, expires_at, revoked_at, last_used_at FROM api_tokens ORDER BY created_at")
        .fetch_all(pool)
        .await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::Duration;

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn tokens_authenticate_until_revoked_or_expired(pool: Pool) {
        let (secret, rec) = create_token(&pool, "ci", "auditor", None)
            .await
            .expect("create");
        assert!(secret.starts_with("apl_") && secret.len() == 68);
        assert_eq!(
            authenticate(&pool, &secret)
                .await
                .expect("auth")
                .map(|r| r.id),
            Some(rec.id)
        );
        assert!(
            list_tokens(&pool).await.expect("list")[0]
                .last_used_at
                .is_some(),
            "use is recorded"
        );
        let stored: Vec<u8> = sqlx::query_scalar("SELECT token_hash FROM api_tokens")
            .fetch_one(&pool)
            .await
            .expect("hash");
        assert_ne!(
            stored,
            secret.as_bytes(),
            "the plaintext token is never stored"
        );
        revoke_token(&pool, rec.id).await.expect("revoke");
        assert_eq!(authenticate(&pool, &secret).await.expect("auth"), None);
        assert!(matches!(
            revoke_token(&pool, rec.id).await,
            Err(StoreError::NotFound(_))
        ));

        let (old, _) = create_token(
            &pool,
            "old",
            "operator",
            Some(OffsetDateTime::now_utc() - Duration::minutes(1)),
        )
        .await
        .expect("create");
        assert_eq!(
            authenticate(&pool, &old).await.expect("auth"),
            None,
            "expired tokens are refused"
        );
        assert_eq!(
            authenticate(&pool, "apl_not-a-real-token")
                .await
                .expect("auth"),
            None
        );
        assert_eq!(list_tokens(&pool).await.expect("list").len(), 2);
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn unknown_roles_are_rejected_by_the_database(pool: Pool) {
        assert!(create_token(&pool, "x", "root", None).await.is_err());
    }
}
