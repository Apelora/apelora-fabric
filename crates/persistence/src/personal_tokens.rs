// SPDX-License-Identifier: AGPL-3.0-only
//! Personal tokens (migration 0026): `apt_` + 64 hex characters, bound to one person and stored
//! only as a SHA-256 hash. A token carries no permissions of its own: whoever presents it acts as
//! its person with what that person holds at the time of the request, and a disabled person's
//! tokens are refused (and revoked when they are disabled). Every check uses the database's clock.

use sqlx::{FromRow, PgConnection};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::tokens::{hash, random_secret};
use crate::{Pool, Result, StoreError};

/// Personal tokens start with this.
pub const PREFIX: &str = "apt_";

/// At most this many live (not revoked, not expired) tokens per person.
pub const MAX_LIVE_PER_PERSON: i64 = 20;

/// A personal token as stored, with its owner's name (never the token itself).
#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct PersonalTokenRecord {
    pub id: Uuid,
    pub user_id: Uuid,
    /// The owner's user name, e-mail address or subject.
    pub owner: String,
    pub name: String,
    pub created_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
    pub last_used_at: Option<OffsetDateTime>,
    pub revoked_at: Option<OffsetDateTime>,
    pub revoked_by: Option<String>,
    pub rotated_from: Option<Uuid>,
    pub rotated_at: Option<OffsetDateTime>,
    /// The token that replaced this one, if it was rotated.
    pub successor: Option<Uuid>,
}

const COLS: &str = "t.id, t.user_id, coalesce(u.username, u.email, u.subject) AS owner, t.name,
    t.created_at, t.expires_at, t.last_used_at, t.revoked_at, t.revoked_by, t.rotated_from,
    t.rotated_at, (SELECT s.id FROM personal_tokens s WHERE s.rotated_from = t.id) AS successor";

/// Whom a presented token belongs to.
#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct PersonalTokenUse {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
}

async fn record(conn: &mut PgConnection, id: Uuid) -> Result<PersonalTokenRecord> {
    sqlx::query_as::<_, PersonalTokenRecord>(&format!(
        "SELECT {COLS} FROM personal_tokens t JOIN users u ON u.id = t.user_id WHERE t.id = $1"
    ))
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| StoreError::NotFound(format!("personal token {id}")))
}

/// Creates a token for `user_id` that expires after `days` (the caller checks 1–365) and
/// returns it (only here) with its record. Refused with `Conflict` when the person already has
/// [`MAX_LIVE_PER_PERSON`] live tokens. Runs inside the caller's transaction.
pub async fn create_in(
    conn: &mut PgConnection,
    user_id: Uuid,
    name: &str,
    days: i64,
) -> Result<(String, PersonalTokenRecord)> {
    create_with(conn, user_id, name, days, None).await
}

async fn create_with(
    conn: &mut PgConnection,
    user_id: Uuid,
    name: &str,
    days: i64,
    rotated_from: Option<Uuid>,
) -> Result<(String, PersonalTokenRecord)> {
    // The person's row lock serialises concurrent creations, so the limit holds.
    sqlx::query("SELECT 1 FROM users WHERE id = $1 FOR UPDATE")
        .bind(user_id)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or_else(|| StoreError::NotFound(format!("user {user_id}")))?;
    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM personal_tokens
         WHERE user_id = $1 AND revoked_at IS NULL AND expires_at > now()",
    )
    .bind(user_id)
    .fetch_one(&mut *conn)
    .await?;
    // A rotation briefly needs one more: the old token lives on for its overlap window.
    let limit = MAX_LIVE_PER_PERSON + i64::from(rotated_from.is_some());
    if live >= limit {
        return Err(StoreError::Conflict(format!(
            "you already have {MAX_LIVE_PER_PERSON} personal tokens; revoke one you no longer use"
        )));
    }
    let token = random_secret(PREFIX)?;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO personal_tokens (id, user_id, name, token_hash, expires_at, rotated_from)
         VALUES ($1, $2, $3, $4, now() + make_interval(days => $5), $6)",
    )
    .bind(id)
    .bind(user_id)
    .bind(name)
    .bind(hash(&token))
    .bind(i32::try_from(days).unwrap_or(i32::MAX))
    .bind(rotated_from)
    .execute(&mut *conn)
    .await?;
    Ok((token, record(conn, id).await?))
}

/// The token a presented secret belongs to, if it is live: not revoked, not expired, and its
/// person is not disabled. Records the use.
pub async fn authenticate(pool: &Pool, presented: &str) -> Result<Option<PersonalTokenUse>> {
    if !presented.starts_with(PREFIX) || presented.len() != PREFIX.len() + 64 {
        return Ok(None);
    }
    Ok(sqlx::query_as::<_, PersonalTokenUse>(
        "UPDATE personal_tokens t SET last_used_at = now() FROM users u
         WHERE t.token_hash = $1 AND u.id = t.user_id AND u.disabled_at IS NULL
           AND t.revoked_at IS NULL AND t.expires_at > now()
         RETURNING t.id, t.user_id, t.name",
    )
    .bind(hash(presented))
    .fetch_optional(pool)
    .await?)
}

/// One person's tokens (or everyone's with `None`), newest first. Tokens that ended more than 30
/// days ago are left out.
pub async fn list(pool: &Pool, user_id: Option<Uuid>) -> Result<Vec<PersonalTokenRecord>> {
    Ok(sqlx::query_as::<_, PersonalTokenRecord>(&format!(
        "SELECT {COLS} FROM personal_tokens t JOIN users u ON u.id = t.user_id
         WHERE ($1::uuid IS NULL OR t.user_id = $1)
           AND coalesce(t.revoked_at, t.expires_at) > now() - interval '30 days'
         ORDER BY t.created_at DESC, t.id"
    ))
    .bind(user_id)
    .fetch_all(pool)
    .await?)
}

/// A token by id, optionally only if it belongs to `user_id`.
pub async fn get(pool: &Pool, id: Uuid, user_id: Option<Uuid>) -> Result<PersonalTokenRecord> {
    let r = record(&mut *pool.acquire().await?, id).await?;
    if user_id.is_some_and(|u| u != r.user_id) {
        return Err(StoreError::NotFound(format!("personal token {id}")));
    }
    Ok(r)
}

/// Revokes a live token (only `user_id`'s, when given). `NotFound` when there is no such live
/// token.
pub async fn revoke(
    db: impl sqlx::PgExecutor<'_>,
    id: Uuid,
    user_id: Option<Uuid>,
    by: &str,
) -> Result<()> {
    let n = sqlx::query(
        "UPDATE personal_tokens SET revoked_at = now(), revoked_by = $3
         WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2)
           AND revoked_at IS NULL AND expires_at > now()",
    )
    .bind(id)
    .bind(user_id)
    .bind(by)
    .execute(db)
    .await?
    .rows_affected();
    if n == 1 {
        Ok(())
    } else {
        Err(StoreError::NotFound(format!("live personal token {id}")))
    }
}

/// Revokes every live token of a person (when they are disabled); returns how many.
pub async fn revoke_all_in(conn: &mut PgConnection, user_id: Uuid, by: &str) -> Result<u64> {
    Ok(sqlx::query(
        "UPDATE personal_tokens SET revoked_at = now(), revoked_by = $2
         WHERE user_id = $1 AND revoked_at IS NULL AND expires_at > now()",
    )
    .bind(user_id)
    .bind(by)
    .execute(conn)
    .await?
    .rows_affected())
}

/// The result of a rotation: the successor's secret (only here) and both records.
pub struct Rotation {
    pub token: String,
    pub successor: PersonalTokenRecord,
    pub previous: PersonalTokenRecord,
}

/// Rotates `user_id`'s live token `id`: a successor with the same name that expires after
/// `days`, and the old token now expires `overlap_secs` from now (or keeps an earlier expiry).
/// `NotFound` for a token that is not the person's or not live; `Conflict` when it was already
/// rotated. Runs inside the caller's transaction.
pub async fn rotate_in(
    conn: &mut PgConnection,
    id: Uuid,
    user_id: Uuid,
    days: i64,
    overlap_secs: i64,
) -> Result<Rotation> {
    let row: Option<(String, Option<OffsetDateTime>)> = sqlx::query_as(
        "SELECT name, rotated_at FROM personal_tokens
         WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL AND expires_at > now()
         FOR UPDATE",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((name, rotated)) = row else {
        return Err(StoreError::NotFound(format!("live personal token {id}")));
    };
    if rotated.is_some() {
        return Err(StoreError::Conflict(
            "this token was already rotated; rotate its successor instead".into(),
        ));
    }
    let (token, successor) = create_with(conn, user_id, &name, days, Some(id)).await?;
    sqlx::query(
        "UPDATE personal_tokens SET rotated_at = now(),
             expires_at = least(expires_at, now() + make_interval(secs => $2))
         WHERE id = $1",
    )
    .bind(id)
    .bind(overlap_secs as f64)
    .execute(&mut *conn)
    .await?;
    let previous = record(conn, id).await?;
    Ok(Rotation {
        token,
        successor,
        previous,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn person(pool: &Pool, name: &str) -> Uuid {
        crate::identity::upsert_user(pool, "https://id.test", name, Some(name), None, None)
            .await
            .expect("user")
            .id
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn tokens_work_until_revoked_expired_or_their_person_is_disabled(pool: Pool) {
        let ann = person(&pool, "ann").await;
        let mut c = pool.acquire().await.expect("conn");
        let (secret, rec) = create_in(&mut c, ann, "laptop", 30).await.expect("create");
        assert!(secret.starts_with("apt_") && secret.len() == 68);
        assert_eq!(rec.owner, "ann");
        let stored: Vec<u8> = sqlx::query_scalar("SELECT token_hash FROM personal_tokens")
            .fetch_one(&pool)
            .await
            .expect("hash");
        assert_ne!(stored, secret.as_bytes(), "only the hash is stored");
        let used = authenticate(&pool, &secret).await.expect("auth");
        assert_eq!(used.map(|u| (u.id, u.user_id)), Some((rec.id, ann)));
        assert!(
            list(&pool, Some(ann)).await.expect("list")[0]
                .last_used_at
                .is_some()
        );

        // Disabled: refused at once; enabled again: works again (revocation is separate).
        sqlx::query("UPDATE users SET disabled_at = now() WHERE id = $1")
            .bind(ann)
            .execute(&pool)
            .await
            .expect("disable");
        assert_eq!(authenticate(&pool, &secret).await.expect("auth"), None);
        sqlx::query("UPDATE users SET disabled_at = NULL WHERE id = $1")
            .bind(ann)
            .execute(&pool)
            .await
            .expect("enable");
        assert!(authenticate(&pool, &secret).await.expect("auth").is_some());

        // Someone else's id is not found; the owner's is revoked once.
        let bob = person(&pool, "bob").await;
        assert!(matches!(
            revoke(&pool, rec.id, Some(bob), "user:bob").await,
            Err(StoreError::NotFound(_))
        ));
        revoke(&pool, rec.id, Some(ann), "user:ann")
            .await
            .expect("revoke");
        assert_eq!(authenticate(&pool, &secret).await.expect("auth"), None);
        assert!(matches!(
            revoke(&pool, rec.id, None, "user:root").await,
            Err(StoreError::NotFound(_))
        ));

        // Expired tokens are refused.
        let (old, rec) = create_in(&mut c, ann, "old", 1).await.expect("create");
        sqlx::query(
            "UPDATE personal_tokens SET expires_at = now() - interval '1 second' WHERE id = $1",
        )
        .bind(rec.id)
        .execute(&pool)
        .await
        .expect("expire");
        assert_eq!(authenticate(&pool, &old).await.expect("auth"), None);
        assert_eq!(authenticate(&pool, "apt_short").await.expect("auth"), None);
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn rotation_makes_a_successor_and_shortens_the_old_token_once(pool: Pool) {
        let ann = person(&pool, "ann").await;
        let mut c = pool.acquire().await.expect("conn");
        let (old, rec) = create_in(&mut c, ann, "ci", 90).await.expect("create");
        let r = rotate_in(&mut c, rec.id, ann, 30, 3600)
            .await
            .expect("rotate");
        assert_eq!(r.successor.name, "ci");
        assert_eq!(r.successor.rotated_from, Some(rec.id));
        assert_eq!(r.previous.successor, Some(r.successor.id));
        let left = r.previous.expires_at - OffsetDateTime::now_utc();
        assert!(left <= time::Duration::hours(1) && left > time::Duration::minutes(59));
        assert!(
            authenticate(&pool, &old).await.expect("auth").is_some(),
            "overlap"
        );
        assert!(authenticate(&pool, &r.token).await.expect("auth").is_some());
        assert!(matches!(
            rotate_in(&mut c, rec.id, ann, 30, 3600).await,
            Err(StoreError::Conflict(_))
        ));
        let bob = person(&pool, "bob").await;
        assert!(matches!(
            rotate_in(&mut c, r.successor.id, bob, 30, 0).await,
            Err(StoreError::NotFound(_))
        ));
        // No overlap: the old token ends now.
        let r2 = rotate_in(&mut c, r.successor.id, ann, 30, 0)
            .await
            .expect("rotate");
        assert_eq!(authenticate(&pool, &r.token).await.expect("auth"), None);
        assert!(
            authenticate(&pool, &r2.token)
                .await
                .expect("auth")
                .is_some()
        );
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn a_person_has_at_most_twenty_live_tokens(pool: Pool) {
        let ann = person(&pool, "ann").await;
        let mut c = pool.acquire().await.expect("conn");
        for i in 0..MAX_LIVE_PER_PERSON {
            create_in(&mut c, ann, &format!("t{i}"), 1)
                .await
                .expect("create");
        }
        assert!(matches!(
            create_in(&mut c, ann, "one too many", 1).await,
            Err(StoreError::Conflict(_))
        ));
        let first = list(&pool, Some(ann)).await.expect("list")[0].id;
        // A rotation may go one over, for the overlap window.
        rotate_in(&mut c, first, ann, 1, 60).await.expect("rotate");
        assert_eq!(
            revoke_all_in(&mut c, ann, "disabled").await.expect("all"),
            21
        );
        create_in(&mut c, ann, "fresh", 1)
            .await
            .expect("room again");
    }
}
