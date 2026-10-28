// SPDX-License-Identifier: AGPL-3.0-only
//! Replicas taken out of their service addresses before Incus moves them (migration 0034).

use sqlx::{FromRow, PgPool};

use crate::Result;

/// How long a drain holds at most: long enough for any evacuation or restore to move the
/// instance, short enough that a drain nobody ended cannot keep a replica out for good.
pub const DRAIN_LIFETIME_SECS: i64 = 15 * 60;

/// One instance kept out of its service addresses while it is on `from_member`.
#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct Drain {
    pub project: String,
    pub instance: String,
    pub from_member: String,
}

/// Drains `instances` (`project`, `instance`, the member it is leaving), replacing any earlier
/// drain of the same instance, and removes drains older than [`DRAIN_LIFETIME_SECS`].
pub async fn drain(pool: &PgPool, instances: &[Drain], by: &str) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "DELETE FROM instance_drains WHERE requested_at < now() - make_interval(secs => $1)",
    )
    .bind(DRAIN_LIFETIME_SECS as f64)
    .execute(&mut *tx)
    .await?;
    for d in instances {
        sqlx::query(
            "INSERT INTO instance_drains (project, instance, from_member, requested_by)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (project, instance) DO UPDATE
             SET from_member = $3, requested_by = $4, requested_at = now()",
        )
        .bind(&d.project)
        .bind(&d.instance)
        .bind(&d.from_member)
        .bind(by)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Ends the drains of these instances (a move that Incus refused).
pub async fn undrain(pool: &PgPool, instances: &[Drain]) -> Result<()> {
    for d in instances {
        sqlx::query(
            "DELETE FROM instance_drains WHERE project = $1 AND instance = $2 AND from_member = $3",
        )
        .bind(&d.project)
        .bind(&d.instance)
        .bind(&d.from_member)
        .execute(pool)
        .await?;
    }
    Ok(())
}

/// Ends the drains of instances leaving `member`; returns how many there were.
pub async fn end_drains_from(pool: &PgPool, member: &str) -> Result<u64> {
    Ok(
        sqlx::query("DELETE FROM instance_drains WHERE from_member = $1")
            .bind(member)
            .execute(pool)
            .await?
            .rows_affected(),
    )
}

/// The drains that still hold (younger than [`DRAIN_LIFETIME_SECS`]). Whether one applies also
/// depends on where the instance is now: only while it is still on `from_member`.
pub async fn active_drains(pool: &PgPool) -> Result<Vec<Drain>> {
    Ok(sqlx::query_as::<_, Drain>(
        "SELECT project, instance, from_member FROM instance_drains
         WHERE requested_at >= now() - make_interval(secs => $1)
         ORDER BY project, instance",
    )
    .bind(DRAIN_LIFETIME_SECS as f64)
    .fetch_all(pool)
    .await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(instance: &str, member: &str) -> Drain {
        Drain {
            project: "apelora".into(),
            instance: instance.into(),
            from_member: member.into(),
        }
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn drains_are_listed_replaced_ended_per_member_and_expire(pool: PgPool) {
        drain(
            &pool,
            &[d("shop-web-01", "m1"), d("shop-web-01-r2", "m2")],
            "alex",
        )
        .await
        .expect("drain");
        assert_eq!(
            active_drains(&pool).await.expect("list"),
            [d("shop-web-01", "m1"), d("shop-web-01-r2", "m2")]
        );
        // Draining again (a restore after an evacuation) replaces the member it leaves.
        drain(&pool, &[d("shop-web-01", "m3")], "alex")
            .await
            .expect("drain again");
        assert_eq!(
            active_drains(&pool).await.expect("list")[0],
            d("shop-web-01", "m3")
        );
        assert_eq!(end_drains_from(&pool, "m2").await.expect("end"), 1);
        // Undoing a drain removes only that one (the same instance leaving that member).
        undrain(&pool, &[d("shop-web-01", "m1")])
            .await
            .expect("undo other member");
        assert_eq!(active_drains(&pool).await.expect("list").len(), 1);
        assert_eq!(
            active_drains(&pool).await.expect("list"),
            [d("shop-web-01", "m3")]
        );
        // An old drain no longer holds, and the next drain removes it.
        sqlx::query("UPDATE instance_drains SET requested_at = now() - interval '1 hour'")
            .execute(&pool)
            .await
            .expect("age");
        assert!(active_drains(&pool).await.expect("list").is_empty());
        drain(&pool, &[], "alex").await.expect("tidy");
        let left: i64 = sqlx::query_scalar("SELECT count(*) FROM instance_drains")
            .fetch_one(&pool)
            .await
            .expect("count");
        assert_eq!(left, 0);
    }
}
