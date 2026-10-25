// SPDX-License-Identifier: AGPL-3.0-only
//! Quotas on organisations, projects and environments, and what Apelora knows about each
//! environment's Incus project (migration 0027; design `docs/design/tenancy-isolation.md`).

use std::collections::BTreeMap;

use apelora_domain::tenancy::{QuotaLimits, QuotaScopeKind, TenancyIncusState};
use serde_json::Value;
use sqlx::postgres::PgPool;
use sqlx::{PgConnection, Row};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::Result;
use crate::identity::{ENV_COLS, EnvironmentRow};

/// One scope's quota.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaRow {
    pub kind: QuotaScopeKind,
    pub scope_id: Uuid,
    pub limits: QuotaLimits,
    pub updated_by: String,
    pub updated_at: OffsetDateTime,
}

fn column(kind: QuotaScopeKind) -> &'static str {
    match kind {
        QuotaScopeKind::Organization => "organization_id",
        QuotaScopeKind::Project => "project_id",
        QuotaScopeKind::Environment => "environment_id",
    }
}

fn quota_row(r: &sqlx::postgres::PgRow) -> QuotaRow {
    let (kind, scope_id) = match (
        r.get::<Option<Uuid>, _>("organization_id"),
        r.get::<Option<Uuid>, _>("project_id"),
        r.get::<Option<Uuid>, _>("environment_id"),
    ) {
        (Some(o), _, _) => (QuotaScopeKind::Organization, o),
        (_, Some(p), _) => (QuotaScopeKind::Project, p),
        (_, _, Some(e)) => (QuotaScopeKind::Environment, e),
        // The table's CHECK makes exactly one of them set.
        (None, None, None) => (QuotaScopeKind::Environment, Uuid::nil()),
    };
    QuotaRow {
        kind,
        scope_id,
        limits: QuotaLimits {
            instances: r.get("instances"),
            cpu: r.get("cpu"),
            memory_gib: r.get("memory_gib"),
            disk_gib: r.get("disk_gib"),
        },
        updated_by: r.get("updated_by"),
        updated_at: r.get("updated_at"),
    }
}

const QUOTA_COLS: &str = "organization_id, project_id, environment_id, instances, cpu, memory_gib, disk_gib, updated_by, updated_at";

/// The quota of one scope, if it has one.
pub async fn quota(
    db: impl sqlx::PgExecutor<'_>,
    kind: QuotaScopeKind,
    id: Uuid,
) -> Result<Option<QuotaRow>> {
    let row = sqlx::query(&format!(
        "SELECT {QUOTA_COLS} FROM tenancy_quotas WHERE {} = $1",
        column(kind)
    ))
    .bind(id)
    .fetch_optional(db)
    .await?;
    Ok(row.as_ref().map(quota_row))
}

/// Every quota (the Tenancy page filters by what the caller may see).
pub async fn all_quotas(pool: &PgPool) -> Result<Vec<QuotaRow>> {
    let rows = sqlx::query(&format!("SELECT {QUOTA_COLS} FROM tenancy_quotas"))
        .fetch_all(pool)
        .await?;
    Ok(rows.iter().map(quota_row).collect())
}

/// Sets (or, with empty limits, removes) a scope's quota inside the caller's transaction, after
/// locking the scope's row so two changes cannot interleave. Returns the quota it replaced.
pub async fn set_quota_in(
    tx: &mut PgConnection,
    kind: QuotaScopeKind,
    id: Uuid,
    limits: &QuotaLimits,
    by: &str,
) -> Result<Option<QuotaLimits>> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("apelora.quota.{}.{id}", kind.as_str()))
        .execute(&mut *tx)
        .await?;
    let before = quota(&mut *tx, kind, id).await?.map(|q| q.limits);
    let col = column(kind);
    if limits.is_empty() {
        sqlx::query(&format!("DELETE FROM tenancy_quotas WHERE {col} = $1"))
            .bind(id)
            .execute(&mut *tx)
            .await?;
    } else if before.is_some() {
        sqlx::query(&format!(
            "UPDATE tenancy_quotas SET instances = $2, cpu = $3, memory_gib = $4, disk_gib = $5, updated_by = $6, updated_at = now() WHERE {col} = $1"
        ))
        .bind(id)
        .bind(limits.instances)
        .bind(limits.cpu)
        .bind(limits.memory_gib)
        .bind(limits.disk_gib)
        .bind(by)
        .execute(&mut *tx)
        .await?;
    } else {
        sqlx::query(&format!(
            "INSERT INTO tenancy_quotas (id, {col}, instances, cpu, memory_gib, disk_gib, updated_by) VALUES ($1, $2, $3, $4, $5, $6, $7)"
        ))
        .bind(Uuid::new_v4())
        .bind(id)
        .bind(limits.instances)
        .bind(limits.cpu)
        .bind(limits.memory_gib)
        .bind(limits.disk_gib)
        .bind(by)
        .execute(&mut *tx)
        .await?;
    }
    Ok(before)
}

/// An environment with the project and organisation above it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentChain {
    pub environment: EnvironmentRow,
    pub project_name: String,
    pub project_slug: String,
    pub organization_id: Uuid,
    pub organization_name: String,
    pub organization_slug: String,
    /// The limit keys Apelora last wrote to the environment's Incus project.
    pub incus_limits: BTreeMap<String, String>,
    pub incus_checked_at: Option<OffsetDateTime>,
    /// A platform administrator named the Incus project when creating the environment, so an
    /// existing project of that name may be adopted (migration 0032).
    pub incus_project_chosen: bool,
}

fn chain_row(r: &sqlx::postgres::PgRow) -> EnvironmentChain {
    let limits: Value = r.get("incus_limits");
    EnvironmentChain {
        environment: EnvironmentRow {
            id: r.get("id"),
            project_id: r.get("project_id"),
            slug: r.get("slug"),
            name: r.get("name"),
            kind: r.get("kind"),
            incus_project: r.get("incus_project"),
            created_at: r.get("created_at"),
            incus_state: r.get("incus_state"),
            incus_detail: r.get("incus_detail"),
        },
        project_name: r.get("project_name"),
        project_slug: r.get("project_slug"),
        organization_id: r.get("organization_id"),
        organization_name: r.get("organization_name"),
        organization_slug: r.get("organization_slug"),
        incus_limits: serde_json::from_value(limits).unwrap_or_default(),
        incus_checked_at: r.get("incus_checked_at"),
        incus_project_chosen: r.get("incus_project_chosen"),
    }
}

fn chain_query(filter: &str) -> String {
    format!(
        "SELECT {ENV_COLS}, e.incus_limits, e.incus_checked_at, e.incus_project_chosen, p.name AS project_name, p.slug AS project_slug,
                p.organization_id, o.name AS organization_name, o.slug AS organization_slug
         FROM environments e JOIN projects p ON p.id = e.project_id JOIN organizations o ON o.id = p.organization_id
         WHERE {filter} ORDER BY o.slug, p.slug, e.slug"
    )
}

pub async fn environment_chain(pool: &PgPool, id: Uuid) -> Result<Option<EnvironmentChain>> {
    let row = sqlx::query(&chain_query("e.id = $1"))
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row.as_ref().map(chain_row))
}

/// Every environment beneath a scope (the environment itself for an environment).
pub async fn environments_under(
    pool: &PgPool,
    kind: QuotaScopeKind,
    id: Uuid,
) -> Result<Vec<EnvironmentChain>> {
    let filter = match kind {
        QuotaScopeKind::Organization => "p.organization_id = $1",
        QuotaScopeKind::Project => "e.project_id = $1",
        QuotaScopeKind::Environment => "e.id = $1",
    };
    let rows = sqlx::query(&chain_query(filter))
        .bind(id)
        .fetch_all(pool)
        .await?;
    Ok(rows.iter().map(chain_row).collect())
}

/// Every environment of the given projects.
pub async fn environments_of_projects(
    pool: &PgPool,
    projects: &[Uuid],
) -> Result<Vec<EnvironmentChain>> {
    let rows = sqlx::query(&chain_query("e.project_id = ANY($1)"))
        .bind(projects)
        .fetch_all(pool)
        .await?;
    Ok(rows.iter().map(chain_row).collect())
}

/// Records the Incus project's state after an attempt to prepare it.
/// Records that a platform administrator named the environment's Incus project explicitly (in
/// the creating transaction), which is what allows adopting an existing project of that name.
pub async fn mark_incus_project_chosen(
    db: impl sqlx::PgExecutor<'_>,
    environment: Uuid,
) -> Result<()> {
    sqlx::query("UPDATE environments SET incus_project_chosen = true WHERE id = $1")
        .bind(environment)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn set_incus_state(
    db: impl sqlx::PgExecutor<'_>,
    environment: Uuid,
    state: TenancyIncusState,
    detail: &str,
) -> Result<()> {
    let detail: String = detail.chars().take(2000).collect();
    sqlx::query(
        "UPDATE environments SET incus_state = $2, incus_detail = $3, incus_checked_at = now() WHERE id = $1",
    )
    .bind(environment)
    .bind(state.as_str())
    .bind(detail)
    .execute(db)
    .await?;
    Ok(())
}

/// Records the limit keys Apelora wrote to the environment's Incus project.
pub async fn set_incus_limits(
    db: impl sqlx::PgExecutor<'_>,
    environment: Uuid,
    limits: &BTreeMap<String, String>,
) -> Result<()> {
    sqlx::query(
        "UPDATE environments SET incus_limits = $2, incus_checked_at = now() WHERE id = $1",
    )
    .bind(environment)
    .bind(serde_json::to_value(limits).unwrap_or(Value::Object(Default::default())))
    .execute(db)
    .await?;
    Ok(())
}

/// Serialises changes to the deploy identity's project list across control planes: a
/// session-level advisory lock on a connection of its own, held across the read and the
/// write in Incus (both short, synchronous calls). Release it with [`unlock_trust`].
pub async fn lock_trust(pool: &PgPool) -> Result<sqlx::pool::PoolConnection<sqlx::Postgres>> {
    let mut conn = pool.acquire().await?;
    sqlx::query("SELECT pg_advisory_lock(hashtextextended('apelora.incus.trust', 0))")
        .execute(&mut *conn)
        .await?;
    Ok(conn)
}

pub async fn unlock_trust(mut conn: sqlx::pool::PoolConnection<sqlx::Postgres>) -> Result<()> {
    sqlx::query("SELECT pg_advisory_unlock(hashtextextended('apelora.incus.trust', 0))")
        .execute(&mut *conn)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn env(pool: &PgPool) -> (Uuid, Uuid, Uuid) {
        let mut tx = crate::begin(pool).await.expect("tx");
        let o = crate::identity::create_organization(&mut *tx, "acme", "Acme")
            .await
            .expect("org");
        let p = crate::identity::create_project(&mut *tx, o.id, "shop", "Shop")
            .await
            .expect("project");
        let e = crate::identity::create_environment(
            &mut *tx,
            p.id,
            "prod",
            "Production",
            apelora_auth::EnvKind::Production,
            Some("acme-shop-prod"),
        )
        .await
        .expect("env");
        crate::commit(tx).await.expect("commit");
        (o.id, p.id, e.id)
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn quotas_are_set_replaced_and_removed_per_scope(pool: PgPool) {
        let (org, project, env) = env(&pool).await;
        let ten = QuotaLimits {
            instances: Some(10),
            ..QuotaLimits::default()
        };
        let mut tx = crate::begin(&pool).await.expect("tx");
        assert_eq!(
            set_quota_in(&mut tx, QuotaScopeKind::Organization, org, &ten, "user:a")
                .await
                .expect("set"),
            None
        );
        let more = QuotaLimits {
            cpu: Some(8),
            ..ten
        };
        assert_eq!(
            set_quota_in(&mut tx, QuotaScopeKind::Organization, org, &more, "user:a")
                .await
                .expect("replace"),
            Some(ten)
        );
        set_quota_in(&mut tx, QuotaScopeKind::Project, project, &ten, "user:b")
            .await
            .expect("project");
        crate::commit(tx).await.expect("commit");
        let q = quota(&pool, QuotaScopeKind::Organization, org)
            .await
            .expect("read")
            .expect("set");
        assert_eq!((q.limits, q.updated_by.as_str()), (more, "user:a"));
        assert_eq!(all_quotas(&pool).await.expect("all").len(), 2);
        let mut tx = crate::begin(&pool).await.expect("tx");
        set_quota_in(
            &mut tx,
            QuotaScopeKind::Organization,
            org,
            &QuotaLimits::default(),
            "user:a",
        )
        .await
        .expect("remove");
        crate::commit(tx).await.expect("commit");
        assert_eq!(
            quota(&pool, QuotaScopeKind::Organization, org)
                .await
                .expect("read"),
            None
        );
        let chain = environment_chain(&pool, env)
            .await
            .expect("chain")
            .expect("exists");
        assert_eq!(
            (
                chain.organization_slug.as_str(),
                chain.project_slug.as_str()
            ),
            ("acme", "shop")
        );
        assert_eq!(chain.environment.incus_state, "pending");
        assert_eq!(
            environments_under(&pool, QuotaScopeKind::Organization, org)
                .await
                .expect("under")
                .len(),
            1
        );
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn the_incus_state_and_written_limits_are_recorded(pool: PgPool) {
        let (_, _, env) = env(&pool).await;
        set_incus_state(&pool, env, TenancyIncusState::Created, "created")
            .await
            .expect("state");
        let limits: BTreeMap<String, String> = [("limits.cpu".to_owned(), "8".to_owned())].into();
        set_incus_limits(&pool, env, &limits).await.expect("limits");
        let chain = environment_chain(&pool, env)
            .await
            .expect("chain")
            .expect("exists");
        assert_eq!(chain.environment.incus_state, "created");
        assert_eq!(chain.incus_limits, limits);
        assert!(chain.incus_checked_at.is_some());
        // The lock can be taken and released.
        let conn = lock_trust(&pool).await.expect("lock");
        unlock_trust(conn).await.expect("unlock");
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn a_quota_needs_one_scope_and_one_limit(pool: PgPool) {
        let (org, project, _) = env(&pool).await;
        let both = sqlx::query("INSERT INTO tenancy_quotas (id, organization_id, project_id, cpu, updated_by) VALUES ($1, $2, $3, 1, 'x')")
            .bind(Uuid::new_v4())
            .bind(org)
            .bind(project)
            .execute(&pool)
            .await;
        assert!(both.is_err(), "two scopes in one row");
        let none = sqlx::query(
            "INSERT INTO tenancy_quotas (id, organization_id, updated_by) VALUES ($1, $2, 'x')",
        )
        .bind(Uuid::new_v4())
        .bind(org)
        .execute(&pool)
        .await;
        assert!(none.is_err(), "a row without a limit");
    }
}
