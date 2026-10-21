// SPDX-License-Identifier: AGPL-3.0-only
//! Apelora sessions after a password or directory sign-in (ADR 0007): `apu_` + 64 hex
//! characters, stored only as a SHA-256 hash. A session ends when it has not been used for the
//! idle limit, at its absolute expiry, when it is revoked (sign-out, password reset or change)
//! or when the person is disabled. Every check uses the database's clock.

use sqlx::{FromRow, PgConnection};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::tokens::{hash, random_secret};
use crate::{Pool, Result};

/// Session tokens start with this.
pub const PREFIX: &str = "apu_";

/// A live session as stored (never contains the token).
#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct SessionRecord {
    pub id: Uuid,
    pub user_id: Uuid,
    /// `password` or `directory`.
    pub method: String,
    pub external_groups: Vec<String>,
    pub expires_at: OffsetDateTime,
}

const COLS: &str = "id, user_id, method, external_groups, expires_at";

/// Creates a session that lasts at most `absolute_secs`; returns the token (only here) and the
/// record. Also forgets sessions that ended more than a week ago.
pub async fn create_session(
    pool: &Pool,
    user_id: Uuid,
    method: &str,
    external_groups: &[String],
    absolute_secs: i64,
) -> Result<(String, SessionRecord)> {
    create_session_in(
        &mut *pool.acquire().await?,
        user_id,
        method,
        external_groups,
        absolute_secs,
    )
    .await
}

/// [`create_session`] inside the caller's transaction (`&mut tx`), so it commits or rolls back
/// with the caller's change and audit row; its own transaction becomes a savepoint.
pub async fn create_session_in(
    conn: &mut sqlx::PgConnection,
    user_id: Uuid,
    method: &str,
    external_groups: &[String],
    absolute_secs: i64,
) -> Result<(String, SessionRecord)> {
    let token = random_secret(PREFIX)?;
    let rec = sqlx::query_as::<_, SessionRecord>(&format!(
        "INSERT INTO user_sessions (id, user_id, token_hash, method, external_groups, expires_at)
         VALUES ($1, $2, $3, $4, $5, now() + make_interval(secs => $6)) RETURNING {COLS}"
    ))
    .bind(Uuid::new_v4())
    .bind(user_id)
    .bind(hash(&token))
    .bind(method)
    .bind(external_groups)
    .bind(absolute_secs as f64)
    .fetch_one(&mut *conn)
    .await?;
    sqlx::query("DELETE FROM user_sessions WHERE expires_at < now() - interval '7 days'")
        .execute(&mut *conn)
        .await?;
    Ok((token, rec))
}

/// The session a presented token belongs to, if it is live: not revoked, before its absolute
/// expiry, used within the last `idle_secs`, and its person is not disabled. Records the use,
/// which keeps an active session alive until its absolute expiry.
pub async fn authenticate_session(
    pool: &Pool,
    presented: &str,
    idle_secs: i64,
) -> Result<Option<SessionRecord>> {
    if !presented.starts_with(PREFIX) || presented.len() != PREFIX.len() + 64 {
        return Ok(None);
    }
    Ok(sqlx::query_as::<_, SessionRecord>(&format!(
        "UPDATE user_sessions s SET last_used_at = now() FROM users u
         WHERE s.token_hash = $1 AND u.id = s.user_id AND u.disabled_at IS NULL
           AND s.revoked_at IS NULL AND s.expires_at > now()
           AND s.last_used_at > now() - make_interval(secs => $2)
         RETURNING {}",
        COLS.split(", ")
            .map(|c| format!("s.{c}"))
            .collect::<Vec<_>>()
            .join(", ")
    ))
    .bind(hash(presented))
    .bind(idle_secs as f64)
    .fetch_optional(pool)
    .await?)
}

/// Revokes one session; false when it was already revoked or unknown.
pub async fn revoke_session(db: impl sqlx::PgExecutor<'_>, id: Uuid) -> Result<bool> {
    Ok(sqlx::query(
        "UPDATE user_sessions SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL",
    )
    .bind(id)
    .execute(db)
    .await?
    .rows_affected()
        == 1)
}

/// Revokes every live session of a person (except `keep`); returns how many.
pub async fn revoke_all(pool: &Pool, user_id: Uuid, keep: Option<Uuid>) -> Result<u64> {
    let mut conn = pool.acquire().await?;
    revoke_all_in(&mut conn, user_id, keep).await
}

pub(crate) async fn revoke_all_in(
    conn: &mut PgConnection,
    user_id: Uuid,
    keep: Option<Uuid>,
) -> Result<u64> {
    Ok(sqlx::query(
        "UPDATE user_sessions SET revoked_at = now()
         WHERE user_id = $1 AND revoked_at IS NULL AND ($2::uuid IS NULL OR id <> $2)",
    )
    .bind(user_id)
    .bind(keep)
    .execute(conn)
    .await?
    .rows_affected())
}

/// A live session as people see it in the Console (never the token).
#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct SessionView {
    pub id: Uuid,
    pub user_id: Uuid,
    /// The person's user name, e-mail address or subject.
    pub person: String,
    /// `password` or `directory`.
    pub method: String,
    pub created_at: OffsetDateTime,
    pub last_used_at: OffsetDateTime,
    /// The absolute limit.
    pub expires_at: OffsetDateTime,
    /// When it ends if it is not used again: the idle limit or the absolute one, whichever is first.
    pub ends_at: OffsetDateTime,
}

/// Live sessions (not revoked, before their absolute expiry, used within `idle_secs`), of one
/// person or of everyone, most recently used first.
pub async fn live_sessions(
    pool: &Pool,
    user_id: Option<Uuid>,
    idle_secs: i64,
) -> Result<Vec<SessionView>> {
    Ok(sqlx::query_as::<_, SessionView>(
        "SELECT s.id, s.user_id, coalesce(u.username, u.email, u.subject) AS person, s.method,
                s.created_at, s.last_used_at, s.expires_at,
                least(s.expires_at, s.last_used_at + make_interval(secs => $2)) AS ends_at
         FROM user_sessions s JOIN users u ON u.id = s.user_id
         WHERE ($1::uuid IS NULL OR s.user_id = $1) AND s.revoked_at IS NULL
           AND s.expires_at > now() AND s.last_used_at > now() - make_interval(secs => $2)
         ORDER BY s.last_used_at DESC, s.id
         LIMIT 500",
    )
    .bind(user_id)
    .bind(idle_secs as f64)
    .fetch_all(pool)
    .await?)
}

/// Revokes one live session (only `user_id`'s, when given); returns its person and method, or
/// `None` when there is no such live session.
pub async fn revoke_session_of(
    db: impl sqlx::PgExecutor<'_>,
    id: Uuid,
    user_id: Option<Uuid>,
) -> Result<Option<(Uuid, String)>> {
    Ok(sqlx::query_as::<_, (Uuid, String)>(
        "UPDATE user_sessions SET revoked_at = now()
         WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2)
           AND revoked_at IS NULL AND expires_at > now()
         RETURNING user_id, method",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(db)
    .await?)
}
