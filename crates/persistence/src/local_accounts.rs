// SPDX-License-Identifier: AGPL-3.0-only
//! Local accounts: people who sign in with a username and password kept by Apelora (ADR 0007).
//!
//! A local account is a `users` row with the issuer [`LOCAL_ISSUER`] and the username as its
//! subject, plus a `local_credentials` row. This module stores and compares only what the
//! control plane hands it: an Argon2id hash (never a password) and the SHA-256 of setup tokens.
//! Times are the database's clock.

use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::identity::UserRow;
use crate::tokens::{hash, random_secret};
use crate::{Pool, Result, StoreError};

/// The issuer of every local account in `users`.
pub const LOCAL_ISSUER: &str = "apelora:local";
/// Setup tokens start with this, so secret scanners can recognise them.
pub const SETUP_PREFIX: &str = "apw_";
/// Failed sign-ins in a row that lock an account.
pub const MAX_FAILURES: i32 = 5;
/// How long a lock lasts, in minutes.
pub const LOCK_MINUTES: i32 = 15;
/// How long a setup link is valid, in hours.
pub const SETUP_HOURS: i32 = 24;

const USER_COLS: &str = "u.id, u.issuer, u.subject, u.username, u.email, u.display_name, u.created_at, u.last_seen_at, u.disabled_at";

/// What a password sign-in needs to know about an account.
#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct LocalLogin {
    pub user_id: Uuid,
    pub username: String,
    pub display_name: Option<String>,
    pub email: Option<String>,
    /// `None` while the setup link has not been used.
    pub password_hash: Option<String>,
    pub disabled: bool,
    /// Locked by failed attempts right now (database clock).
    pub locked: bool,
}

const LOGIN_SELECT: &str =
    "SELECT u.id AS user_id, u.subject AS username, u.display_name, u.email, c.password_hash,
        u.disabled_at IS NOT NULL AS disabled, COALESCE(c.locked_until > now(), false) AS locked
    FROM local_credentials c JOIN users u ON u.id = c.user_id";

/// A setup link as handed out once: the token, and until when it works.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupLink {
    pub token: String,
    pub expires_at: OffsetDateTime,
}

fn conflict(e: sqlx::Error) -> StoreError {
    match &e {
        sqlx::Error::Database(d) if d.is_unique_violation() => {
            StoreError::Conflict("a local account with that username already exists".into())
        }
        _ => StoreError::Database(e),
    }
}

/// Creates a local account without a password and returns it with a one-time setup link.
/// `username` must already be lowercase and valid. Refused when any person already has that
/// username, so two people never show up under one name.
pub async fn create_local_account(
    pool: &Pool,
    username: &str,
    display_name: Option<&str>,
    email: Option<&str>,
    by: &str,
) -> Result<(UserRow, SetupLink)> {
    create_local_account_in(
        &mut *pool.acquire().await?,
        username,
        display_name,
        email,
        by,
    )
    .await
}

/// [`create_local_account`] inside the caller's transaction (`&mut tx`), so it commits or rolls back
/// with the caller's change and audit row; its own transaction becomes a savepoint.
pub async fn create_local_account_in(
    conn: &mut sqlx::PgConnection,
    username: &str,
    display_name: Option<&str>,
    email: Option<&str>,
    by: &str,
) -> Result<(UserRow, SetupLink)> {
    let token = random_secret(SETUP_PREFIX)?;
    let mut tx = sqlx::Connection::begin(&mut *conn).await?;
    let taken: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE lower(username) = $1)")
            .bind(username)
            .fetch_one(&mut *tx)
            .await?;
    if taken {
        return Err(StoreError::Conflict(
            "someone already has that username".into(),
        ));
    }
    let user: UserRow = sqlx::query_as(&format!(
        "INSERT INTO users AS u (id, issuer, subject, username, email, display_name) VALUES ($1, $2, $3, $3, $4, $5)
         RETURNING {USER_COLS}"
    ))
    .bind(Uuid::new_v4())
    .bind(LOCAL_ISSUER)
    .bind(username)
    .bind(email)
    .bind(display_name)
    .fetch_one(&mut *tx)
    .await
    .map_err(conflict)?;
    let expires_at: OffsetDateTime = sqlx::query_scalar(
        "INSERT INTO local_credentials (user_id, setup_token_hash, setup_expires_at, created_by)
         VALUES ($1, $2, now() + make_interval(hours => $3), $4) RETURNING setup_expires_at",
    )
    .bind(user.id)
    .bind(hash(&token))
    .bind(SETUP_HOURS)
    .bind(by)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok((user, SetupLink { token, expires_at }))
}

/// Takes a local account's password away, issues a new setup link and revokes every session.
/// Returns the link and how many sessions were revoked. `NotFound` for anyone who is not a
/// local account.
pub async fn reset_local_account(pool: &Pool, user_id: Uuid) -> Result<(SetupLink, u64)> {
    reset_local_account_in(&mut *pool.acquire().await?, user_id).await
}

/// [`reset_local_account`] inside the caller's transaction (`&mut tx`), so it commits or rolls back
/// with the caller's change and audit row; its own transaction becomes a savepoint.
pub async fn reset_local_account_in(
    conn: &mut sqlx::PgConnection,
    user_id: Uuid,
) -> Result<(SetupLink, u64)> {
    let token = random_secret(SETUP_PREFIX)?;
    let mut tx = sqlx::Connection::begin(&mut *conn).await?;
    let expires_at: Option<OffsetDateTime> = sqlx::query_scalar(
        "UPDATE local_credentials SET password_hash = NULL, setup_token_hash = $2,
             setup_expires_at = now() + make_interval(hours => $3), failed_attempts = 0, locked_until = NULL
         WHERE user_id = $1 RETURNING setup_expires_at",
    )
    .bind(user_id)
    .bind(hash(&token))
    .bind(SETUP_HOURS)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(expires_at) = expires_at else {
        return Err(StoreError::NotFound(format!("local account {user_id}")));
    };
    let revoked = crate::user_sessions::revoke_all_in(&mut tx, user_id, None).await?;
    tx.commit().await?;
    Ok((SetupLink { token, expires_at }, revoked))
}

/// The account a setup token belongs to, while the token is unused and unexpired.
pub async fn setup_target(pool: &Pool, token: &str) -> Result<Option<LocalLogin>> {
    if !token.starts_with(SETUP_PREFIX) || token.len() != SETUP_PREFIX.len() + 64 {
        return Ok(None);
    }
    Ok(sqlx::query_as(&format!(
        "{LOGIN_SELECT} WHERE c.setup_token_hash = $1 AND c.setup_expires_at > now()"
    ))
    .bind(hash(token))
    .fetch_optional(pool)
    .await?)
}

/// Sets the password chosen through a setup link and spends the link. `None` when the link was
/// spent or expired in the meantime (single use even under concurrent requests). Revokes any
/// session the account still had.
pub async fn complete_setup(pool: &Pool, token: &str, password_hash: &str) -> Result<Option<Uuid>> {
    complete_setup_in(&mut *pool.acquire().await?, token, password_hash).await
}

/// [`complete_setup`] inside the caller's transaction (`&mut tx`), so it commits or rolls back
/// with the caller's change and audit row; its own transaction becomes a savepoint.
pub async fn complete_setup_in(
    conn: &mut sqlx::PgConnection,
    token: &str,
    password_hash: &str,
) -> Result<Option<Uuid>> {
    let mut tx = sqlx::Connection::begin(&mut *conn).await?;
    let user: Option<Uuid> = sqlx::query_scalar(
        "UPDATE local_credentials SET password_hash = $2, setup_token_hash = NULL, setup_expires_at = NULL,
             password_changed_at = now(), failed_attempts = 0, locked_until = NULL
         WHERE setup_token_hash = $1 AND setup_expires_at > now() RETURNING user_id",
    )
    .bind(hash(token))
    .bind(password_hash)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(id) = user {
        crate::user_sessions::revoke_all_in(&mut tx, id, None).await?;
    }
    tx.commit().await?;
    Ok(user)
}

/// The local account with this (lowercase) username.
pub async fn login_by_username(pool: &Pool, username: &str) -> Result<Option<LocalLogin>> {
    Ok(sqlx::query_as(&format!(
        "{LOGIN_SELECT} WHERE u.issuer = $1 AND u.subject = $2"
    ))
    .bind(LOCAL_ISSUER)
    .bind(username)
    .fetch_optional(pool)
    .await?)
}

/// The local account of this person, if they have one.
pub async fn login_by_id(pool: &Pool, user_id: Uuid) -> Result<Option<LocalLogin>> {
    Ok(
        sqlx::query_as(&format!("{LOGIN_SELECT} WHERE c.user_id = $1"))
            .bind(user_id)
            .fetch_optional(pool)
            .await?,
    )
}

/// Counts a failed attempt. The fifth failure in a row locks the account for
/// [`LOCK_MINUTES`] and starts the count again; returns true when this failure locked it.
/// Attempts while locked are not counted (the lock is not extended by an attacker).
pub async fn record_failure(pool: &Pool, user_id: Uuid) -> Result<bool> {
    let locked: Option<bool> = sqlx::query_scalar(
        "UPDATE local_credentials SET
             failed_attempts = CASE WHEN failed_attempts + 1 >= $2 THEN 0 ELSE failed_attempts + 1 END,
             locked_until = CASE WHEN failed_attempts + 1 >= $2 THEN now() + make_interval(mins => $3) ELSE locked_until END
         WHERE user_id = $1 AND (locked_until IS NULL OR locked_until <= now())
         RETURNING failed_attempts = 0",
    )
    .bind(user_id)
    .bind(MAX_FAILURES)
    .bind(LOCK_MINUTES)
    .fetch_optional(pool)
    .await?;
    Ok(locked.unwrap_or(false))
}

/// A successful sign-in: clears the failure count and any expired lock, and records the visit.
pub async fn record_success(pool: &Pool, user_id: Uuid) -> Result<()> {
    record_success_in(&mut *pool.acquire().await?, user_id).await
}

/// [`record_success`] inside the caller's transaction (`&mut tx`), so it commits or rolls back
/// with the caller's change and audit row; its own transaction becomes a savepoint.
pub async fn record_success_in(conn: &mut sqlx::PgConnection, user_id: Uuid) -> Result<()> {
    sqlx::query(
        "UPDATE local_credentials SET failed_attempts = 0, locked_until = NULL WHERE user_id = $1",
    )
    .bind(user_id)
    .execute(&mut *conn)
    .await?;
    sqlx::query("UPDATE users SET last_seen_at = now() WHERE id = $1")
        .bind(user_id)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// Replaces the password and revokes every session except `keep` (the one in use). Returns how
/// many sessions were revoked.
pub async fn change_password(
    pool: &Pool,
    user_id: Uuid,
    password_hash: &str,
    keep: Option<Uuid>,
) -> Result<u64> {
    change_password_in(&mut *pool.acquire().await?, user_id, password_hash, keep).await
}

/// [`change_password`] inside the caller's transaction (`&mut tx`), so it commits or rolls back
/// with the caller's change and audit row; its own transaction becomes a savepoint.
pub async fn change_password_in(
    conn: &mut sqlx::PgConnection,
    user_id: Uuid,
    password_hash: &str,
    keep: Option<Uuid>,
) -> Result<u64> {
    let mut tx = sqlx::Connection::begin(&mut *conn).await?;
    let n = sqlx::query(
        "UPDATE local_credentials SET password_hash = $2, password_changed_at = now(), failed_attempts = 0,
             locked_until = NULL, setup_token_hash = NULL, setup_expires_at = NULL
         WHERE user_id = $1",
    )
    .bind(user_id)
    .bind(password_hash)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if n == 0 {
        return Err(StoreError::NotFound(format!("local account {user_id}")));
    }
    let revoked = crate::user_sessions::revoke_all_in(&mut tx, user_id, keep).await?;
    tx.commit().await?;
    Ok(revoked)
}

/// Local accounts that can sign in now: a password is set and the person is not disabled.
/// The lockout guards count these as a way in.
pub async fn ready_count(pool: &Pool) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM local_credentials c JOIN users u ON u.id = c.user_id
         WHERE c.password_hash IS NOT NULL AND u.disabled_at IS NULL",
    )
    .fetch_one(pool)
    .await?)
}

/// Local accounts whose setup link has not been used yet.
pub async fn pending_count(pool: &Pool) -> Result<i64> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM local_credentials WHERE password_hash IS NULL")
            .fetch_one(pool)
            .await?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn the_database_refuses_bad_usernames_and_counts_failures_only_while_unlocked(
        pool: Pool,
    ) {
        assert!(
            create_local_account(&pool, "Not Valid", None, None, "test")
                .await
                .is_err(),
            "the CHECK constraint holds even if a caller skips validation"
        );
        let (user, link) = create_local_account(&pool, "nina", None, None, "test")
            .await
            .expect("create");
        assert!(link.token.starts_with(SETUP_PREFIX));
        assert_eq!(ready_count(&pool).await.expect("count"), 0);
        assert_eq!(pending_count(&pool).await.expect("count"), 1);
        assert_eq!(
            complete_setup(&pool, &link.token, "$argon2id$test")
                .await
                .expect("setup"),
            Some(user.id)
        );
        assert_eq!(
            complete_setup(&pool, &link.token, "$argon2id$x")
                .await
                .expect("again"),
            None
        );
        assert_eq!(ready_count(&pool).await.expect("count"), 1);
        for _ in 0..4 {
            assert!(!record_failure(&pool, user.id).await.expect("fail"));
        }
        assert!(
            record_failure(&pool, user.id).await.expect("fail"),
            "the fifth locks"
        );
        assert!(
            !record_failure(&pool, user.id).await.expect("fail"),
            "not counted while locked"
        );
        let login = login_by_username(&pool, "nina")
            .await
            .expect("login")
            .expect("found");
        assert!(login.locked);
        let n: i32 = sqlx::query_scalar("SELECT failed_attempts FROM local_credentials")
            .fetch_one(&pool)
            .await
            .expect("count");
        assert_eq!(n, 0, "the count starts again after a lock");
    }
}
