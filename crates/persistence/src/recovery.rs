// SPDX-License-Identifier: AGPL-3.0-only
//! Failed members, their fencing and the recovery decision (HA slice B, ADR 0013, migration
//! 0028). The supervisor leader writes inside its fenced transactions (`&mut PgConnection`
//! forms); the API reads, and records a person's confirmation.

use serde_json::Value;
use sqlx::{FromRow, PgConnection};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{Pool, Result};

#[derive(Debug, Clone, PartialEq, FromRow)]
pub struct FailureRecord {
    pub id: Uuid,
    pub member: String,
    pub detected_at: OffsetDateTime,
    pub last_heartbeat: Option<OffsetDateTime>,
    pub fencing: Option<Value>,
    pub fencing_at: Option<OffsetDateTime>,
    pub waiting_instances: i32,
    pub fenced_at: Option<OffsetDateTime>,
    pub fence_evidence: Option<Value>,
    pub confirmed_by: Option<String>,
    pub confirmed_at: Option<OffsetDateTime>,
    pub decided_at: Option<OffsetDateTime>,
    pub decision: Option<Value>,
    pub resolved_at: Option<OffsetDateTime>,
}

const COLUMNS: &str = "id, member, detected_at, last_heartbeat, fencing, fencing_at, waiting_instances, fenced_at, fence_evidence, confirmed_by, confirmed_at, decided_at, decision, resolved_at";

/// The open failure of `member`, created when there is none. `true` when it was created now.
pub async fn open_failure(
    conn: &mut PgConnection,
    member: &str,
    last_heartbeat: Option<OffsetDateTime>,
) -> Result<(FailureRecord, bool)> {
    let created = sqlx::query(
        "INSERT INTO member_failures (id, member, last_heartbeat) VALUES ($1, $2, $3)
         ON CONFLICT (member) WHERE resolved_at IS NULL DO NOTHING",
    )
    .bind(Uuid::new_v4())
    .bind(member)
    .bind(last_heartbeat)
    .execute(&mut *conn)
    .await?
    .rows_affected()
        == 1;
    let row = sqlx::query_as::<_, FailureRecord>(&format!(
        "SELECT {COLUMNS} FROM member_failures WHERE member = $1 AND resolved_at IS NULL"
    ))
    .bind(member)
    .fetch_one(&mut *conn)
    .await?;
    Ok((row, created))
}

/// Every failure not yet resolved, oldest first.
pub async fn open_failures(pool: &Pool) -> Result<Vec<FailureRecord>> {
    Ok(sqlx::query_as::<_, FailureRecord>(&format!(
        "SELECT {COLUMNS} FROM member_failures WHERE resolved_at IS NULL ORDER BY detected_at"
    ))
    .fetch_all(pool)
    .await?)
}

/// One failure by id.
pub async fn failure(pool: &Pool, id: Uuid) -> Result<Option<FailureRecord>> {
    Ok(sqlx::query_as::<_, FailureRecord>(&format!(
        "SELECT {COLUMNS} FROM member_failures WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await?)
}

/// Closes a failure: the member is Online again.
pub async fn resolve_failure(conn: &mut PgConnection, id: Uuid) -> Result<bool> {
    Ok(sqlx::query(
        "UPDATE member_failures SET resolved_at = now() WHERE id = $1 AND resolved_at IS NULL",
    )
    .bind(id)
    .execute(conn)
    .await?
    .rows_affected()
        == 1)
}

/// The latest fencing state (a `FenceView`) and how many instances wait for it.
pub async fn set_fencing(
    conn: &mut PgConnection,
    id: Uuid,
    fencing: &Value,
    waiting_instances: i32,
) -> Result<()> {
    sqlx::query(
        "UPDATE member_failures SET fencing = $2, fencing_at = now(), waiting_instances = $3
         WHERE id = $1",
    )
    .bind(id)
    .bind(fencing)
    .bind(waiting_instances)
    .execute(conn)
    .await?;
    Ok(())
}

/// Records that the member was fenced, with the evidence, once. `false` when it already was.
pub async fn mark_fenced(conn: &mut PgConnection, id: Uuid, evidence: &Value) -> Result<bool> {
    Ok(sqlx::query(
        "UPDATE member_failures SET fenced_at = now(), fence_evidence = $2
         WHERE id = $1 AND fenced_at IS NULL AND resolved_at IS NULL",
    )
    .bind(id)
    .bind(evidence)
    .execute(conn)
    .await?
    .rows_affected()
        == 1)
}

/// Records the recovery decision, once. `false` when one was already recorded.
pub async fn record_decision(conn: &mut PgConnection, id: Uuid, decision: &Value) -> Result<bool> {
    Ok(sqlx::query(
        "UPDATE member_failures SET decided_at = now(), decision = $2
         WHERE id = $1 AND decided_at IS NULL AND fenced_at IS NOT NULL",
    )
    .bind(id)
    .bind(decision)
    .execute(conn)
    .await?
    .rows_affected()
        == 1)
}

/// A person confirmed that `member` is powered off: recorded on its open failure (created when
/// the supervisor has not opened one yet).
pub async fn confirm_powered_off(
    conn: &mut PgConnection,
    member: &str,
    by: &str,
) -> Result<FailureRecord> {
    let (row, _) = open_failure(&mut *conn, member, None).await?;
    Ok(sqlx::query_as::<_, FailureRecord>(&format!(
        "UPDATE member_failures SET confirmed_by = $2, confirmed_at = now() WHERE id = $1
         RETURNING {COLUMNS}"
    ))
    .bind(row.id)
    .bind(by)
    .fetch_one(conn)
    .await?)
}

/// A member's host lease as learned while it was Online.
#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct LockHostRecord {
    pub member: String,
    pub lockspace: String,
    pub host_id: i32,
    pub generation: i64,
    pub observed_at: OffsetDateTime,
}

pub async fn lock_hosts(pool: &Pool) -> Result<Vec<LockHostRecord>> {
    Ok(sqlx::query_as::<_, LockHostRecord>(
        "SELECT member, lockspace, host_id, generation, observed_at FROM member_lock_hosts ORDER BY member",
    )
    .fetch_all(pool)
    .await?)
}

/// Records (or refreshes) a member's lease.
pub async fn learn_lock_host(
    conn: &mut PgConnection,
    member: &str,
    lockspace: &str,
    host_id: i32,
    generation: i64,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO member_lock_hosts (member, lockspace, host_id, generation) VALUES ($1, $2, $3, $4)
         ON CONFLICT (member) DO UPDATE SET lockspace = EXCLUDED.lockspace, host_id = EXCLUDED.host_id,
           generation = EXCLUDED.generation, observed_at = now()",
    )
    .bind(member)
    .bind(lockspace)
    .bind(host_id)
    .bind(generation)
    .execute(conn)
    .await?;
    Ok(())
}

/// Open failures that keep instances waiting for fencing, with how long (alert
/// `member-unfenced`): member, waiting instances, seconds since detected.
pub async fn unfenced(pool: &Pool) -> Result<Vec<(String, i32, f64)>> {
    Ok(sqlx::query_as::<_, (String, i32, f64)>(
        "SELECT member, waiting_instances, extract(epoch FROM now() - detected_at)::float8
         FROM member_failures
         WHERE resolved_at IS NULL AND fenced_at IS NULL AND waiting_instances > 0
         ORDER BY member",
    )
    .fetch_all(pool)
    .await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn one_open_failure_per_member_fenced_and_decided_once(pool: Pool) {
        let mut c = pool.acquire().await.expect("conn");
        let (a, created) = open_failure(&mut c, "m2", None).await.expect("open");
        assert!(created);
        let (b, again) = open_failure(&mut c, "m2", None).await.expect("open again");
        assert_eq!((a.id, again), (b.id, false), "one open row per member");
        assert!(
            mark_fenced(&mut c, a.id, &json!({"kind": "storage-lease"}))
                .await
                .expect("fence")
        );
        assert!(
            !mark_fenced(&mut c, a.id, &json!({"kind": "confirmed"}))
                .await
                .expect("again")
        );
        assert!(
            record_decision(&mut c, a.id, &json!([]))
                .await
                .expect("decide")
        );
        assert!(
            !record_decision(&mut c, a.id, &json!([1]))
                .await
                .expect("not twice")
        );
        set_fencing(&mut c, a.id, &json!({"condition": "fenced"}), 0)
            .await
            .expect("state");
        assert!(resolve_failure(&mut c, a.id).await.expect("resolve"));
        assert!(open_failures(&pool).await.expect("list").is_empty());
        let (n, created) = open_failure(&mut c, "m2", None)
            .await
            .expect("a new failure");
        assert!(created && n.id != a.id);
        let confirmed = confirm_powered_off(&mut c, "m2", "user:ana")
            .await
            .expect("confirm");
        assert_eq!(
            (confirmed.id, confirmed.confirmed_by.as_deref()),
            (n.id, Some("user:ana"))
        );
        assert!(
            unfenced(&pool).await.expect("unfenced").is_empty(),
            "nothing waits"
        );
        set_fencing(&mut c, n.id, &json!({}), 2)
            .await
            .expect("state");
        assert_eq!(unfenced(&pool).await.expect("unfenced")[0].1, 2);
        learn_lock_host(&mut c, "m2", "lvm_x", 2, 1)
            .await
            .expect("learn");
        learn_lock_host(&mut c, "m2", "lvm_x", 2, 3)
            .await
            .expect("refresh");
        let hosts = lock_hosts(&pool).await.expect("hosts");
        assert_eq!((hosts.len(), hosts[0].generation), (1, 3));
    }
}
