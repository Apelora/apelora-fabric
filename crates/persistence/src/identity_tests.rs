// Identity and authorisation storage, against a real PostgreSQL (see tests.rs for how to run).

use apelora_auth::{
    AccessFilter, AssignmentKind, Builtin, Conditions, EnvKind, PrincipalRef, Resource, RoleRef,
    Scope,
};
use std::collections::BTreeSet;

use serde_json::json;
use time::{Duration, OffsetDateTime};

use super::identity::*;
use super::*;

fn assignment(principal: PrincipalRef, role: Builtin, scope: Scope) -> NewAssignment {
    NewAssignment {
        principal,
        role: RoleRef::Builtin {
            key: role.key().into(),
        },
        scope,
        conditions: Conditions::default(),
        kind: AssignmentKind::Standard,
        expires_at: None,
        reason: None,
        created_by: "test".into(),
    }
}

struct Tenants {
    acme: OrganizationRow,
    acme_prod: ProjectRow,
    other: OrganizationRow,
    other_proj: ProjectRow,
}

async fn tenants(pool: &PgPool) -> Tenants {
    let acme = create_organization(pool, "acme", "Acme")
        .await
        .expect("org");
    let other = create_organization(pool, "other", "Other")
        .await
        .expect("org");
    let acme_prod = create_project(pool, acme.id, "prod", "Production")
        .await
        .expect("project");
    let other_proj = create_project(pool, other.id, "web", "Web")
        .await
        .expect("project");
    Tenants {
        acme,
        acme_prod,
        other,
        other_proj,
    }
}

#[sqlx::test(migrator = "crate::MIGRATOR")]
async fn existing_applications_land_in_the_default_project_and_environment(pool: PgPool) {
    let app = create_application(&pool, "legacy").await.expect("app");
    let ctx = application_context(&pool, "legacy")
        .await
        .expect("query")
        .expect("context");
    assert_eq!(
        (ctx.application_id, ctx.project_id, ctx.organization_id),
        (app.id, DEFAULT_PROJECT, DEFAULT_ORGANIZATION)
    );
    let (env, org) = environment_by_incus_project(&pool, "apelora")
        .await
        .expect("query")
        .expect("default environment");
    assert_eq!(
        (env.id, org),
        (DEFAULT_ENVIRONMENT, DEFAULT_ORGANIZATION),
        "the Incus project Apelora always deployed into"
    );
}

#[sqlx::test(migrator = "crate::MIGRATOR")]
async fn tenancy_rows_are_unique_validated_and_protected(pool: PgPool) {
    let t = tenants(&pool).await;
    assert!(matches!(
        create_organization(&pool, "acme", "Again").await,
        Err(StoreError::Conflict(_))
    ));
    assert!(
        create_organization(&pool, "Bad Slug", "x").await.is_err(),
        "slug shape is enforced by the database"
    );
    assert!(matches!(
        create_project(&pool, t.acme.id, "prod", "Again").await,
        Err(StoreError::Conflict(_))
    ));
    create_project(&pool, t.other.id, "prod", "Same slug, other organisation")
        .await
        .expect("slugs are per organisation");
    let env = create_environment(
        &pool,
        t.acme_prod.id,
        "live",
        "Live",
        EnvKind::Production,
        Some("acme-prod-live"),
    )
    .await
    .expect("env");
    assert_eq!(env.kind, "production");
    assert!(
        matches!(
            create_environment(
                &pool,
                t.other_proj.id,
                "xx",
                "XX",
                EnvKind::Other,
                Some("acme-prod-live")
            )
            .await,
            Err(StoreError::Conflict(_))
        ),
        "an Incus project belongs to one environment"
    );
    let sql = sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(t.acme.id)
        .execute(&pool)
        .await;
    assert!(
        sql.is_err(),
        "an organisation with projects cannot be deleted"
    );
}

#[sqlx::test(migrator = "crate::MIGRATOR")]
async fn lists_are_scoped_to_what_the_filter_reaches(pool: PgPool) {
    let t = tenants(&pool).await;
    let a = ensure_application_in(&pool, "shop", t.acme_prod.id, None)
        .await
        .expect("app");
    ensure_application_in(&pool, "intranet", t.other_proj.id, None)
        .await
        .expect("app");
    let names = |v: Vec<ApplicationSummary>| v.into_iter().map(|a| a.name).collect::<Vec<_>>();

    assert_eq!(
        names(
            list_applications_scoped(&pool, &AccessFilter::All)
                .await
                .expect("list")
        ),
        vec!["intranet", "shop"]
    );
    let by_org = AccessFilter::Limited {
        organizations: vec![t.acme.id],
        projects: vec![],
        applications: vec![],
    };
    assert_eq!(
        names(
            list_applications_scoped(&pool, &by_org)
                .await
                .expect("list")
        ),
        vec!["shop"]
    );
    let by_project = AccessFilter::Limited {
        organizations: vec![],
        projects: vec![t.other_proj.id],
        applications: vec![],
    };
    assert_eq!(
        names(
            list_applications_scoped(&pool, &by_project)
                .await
                .expect("list")
        ),
        vec!["intranet"]
    );
    let by_app = AccessFilter::Limited {
        organizations: vec![],
        projects: vec![],
        applications: vec![a.id],
    };
    assert_eq!(
        names(
            list_applications_scoped(&pool, &by_app)
                .await
                .expect("list")
        ),
        vec!["shop"]
    );
    let none = AccessFilter::Limited {
        organizations: vec![],
        projects: vec![],
        applications: vec![],
    };
    assert!(
        list_applications_scoped(&pool, &none)
            .await
            .expect("list")
            .is_empty(),
        "an empty filter reaches nothing"
    );

    assert_eq!(
        list_organizations(&pool, &by_project)
            .await
            .expect("orgs")
            .iter()
            .map(|o| o.slug.as_str())
            .collect::<Vec<_>>(),
        vec!["other"],
        "an organisation is visible through a project grant"
    );
    assert!(
        list_projects(&pool, &none)
            .await
            .expect("projects")
            .is_empty()
    );
    assert_eq!(
        list_projects(&pool, &by_app)
            .await
            .expect("projects")
            .iter()
            .map(|p| p.slug.as_str())
            .collect::<Vec<_>>(),
        vec!["prod"]
    );
}

#[sqlx::test(migrator = "crate::MIGRATOR")]
async fn an_application_name_cannot_be_taken_over_from_another_project(pool: PgPool) {
    let t = tenants(&pool).await;
    ensure_application_in(&pool, "shop", t.acme_prod.id, None)
        .await
        .expect("app");
    ensure_application_in(&pool, "shop", t.acme_prod.id, None)
        .await
        .expect("repeat in the same project is fine");
    assert!(matches!(
        ensure_application_in(&pool, "shop", t.other_proj.id, None).await,
        Err(StoreError::Conflict(_))
    ));
}

#[sqlx::test(migrator = "crate::MIGRATOR")]
async fn assignments_load_into_the_engine_until_revoked_or_expired(pool: PgPool) {
    let t = tenants(&pool).await;
    let u = upsert_user(
        &pool,
        "https://id.test",
        "sub-1",
        Some("alice"),
        Some("a@test"),
        Some("Alice"),
    )
    .await
    .expect("user");
    let same = upsert_user(
        &pool,
        "https://id.test",
        "sub-1",
        Some("alice2"),
        None,
        None,
    )
    .await
    .expect("user");
    assert_eq!(
        (u.id, same.username.as_deref()),
        (same.id, Some("alice2")),
        "the same subject is the same user"
    );

    let me = PrincipalRef::User { id: u.id };
    let id = insert_assignment(
        &pool,
        &assignment(
            me,
            Builtin::ProjectAdministrator,
            Scope::Project { id: t.acme_prod.id },
        ),
    )
    .await
    .expect("assign");
    let mut expiring = assignment(me, Builtin::Viewer, Scope::Organization { id: t.other.id });
    expiring.expires_at = Some(OffsetDateTime::now_utc() - Duration::minutes(1));
    insert_assignment(&pool, &expiring).await.expect("assign");

    let engine = load_engine(&pool, &[me]).await.expect("engine");
    assert_eq!(engine.assignments.len(), 1, "the expired one is not loaded");
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let subject = apelora_auth::Subject {
        user: Some(u.id),
        ..Default::default()
    };
    let res = Resource::project(t.acme.id, t.acme_prod.id);
    assert!(
        engine
            .check(&subject, "application.create", &res, now)
            .allowed
    );

    revoke_assignment(&pool, id, "test").await.expect("revoke");
    assert!(
        load_engine(&pool, &[me])
            .await
            .expect("engine")
            .assignments
            .is_empty(),
        "revocation takes effect on the next request"
    );
    assert!(
        matches!(
            revoke_assignment(&pool, id, "test").await,
            Err(StoreError::NotFound(_))
        ),
        "revoking twice"
    );
}

#[sqlx::test(migrator = "crate::MIGRATOR")]
async fn group_grants_reach_members_by_membership_and_by_token_group_name(pool: PgPool) {
    let t = tenants(&pool).await;
    let u = upsert_user(&pool, "https://id.test", "sub-2", Some("bob"), None, None)
        .await
        .expect("user");
    let local = create_group(&pool, Some(t.acme.id), "Operators", None, "")
        .await
        .expect("group");
    let idp = create_group(&pool, None, "Platform team", Some("platform"), "")
        .await
        .expect("group");
    assert!(matches!(
        create_group(&pool, Some(t.acme.id), "Operators", None, "").await,
        Err(StoreError::Conflict(_))
    ));
    assert!(
        matches!(
            create_group(&pool, None, "Another", Some("platform"), "").await,
            Err(StoreError::Conflict(_))
        ),
        "an identity-provider name maps to one group"
    );

    assert!(
        groups_of(&pool, u.id, &BTreeSet::new())
            .await
            .expect("groups")
            .is_empty()
    );
    add_group_member(&pool, local.id, u.id, "test")
        .await
        .expect("member");
    let token_groups: BTreeSet<String> = ["platform".to_string(), "unknown".to_string()].into();
    let got = groups_of(&pool, u.id, &token_groups).await.expect("groups");
    assert_eq!(got, [local.id, idp.id].into_iter().collect());

    insert_assignment(
        &pool,
        &assignment(
            PrincipalRef::Group { id: local.id },
            Builtin::ApplicationOperator,
            Scope::Project { id: t.acme_prod.id },
        ),
    )
    .await
    .expect("assign");
    let principals: Vec<PrincipalRef> = std::iter::once(PrincipalRef::User { id: u.id })
        .chain(got.iter().map(|g| PrincipalRef::Group { id: *g }))
        .collect();
    assert_eq!(
        load_engine(&pool, &principals)
            .await
            .expect("engine")
            .assignments
            .len(),
        1
    );
    remove_group_member(&pool, local.id, u.id)
        .await
        .expect("remove");
    assert_eq!(
        groups_of(&pool, u.id, &BTreeSet::new())
            .await
            .expect("groups")
            .len(),
        0,
        "leaving the group ends the access"
    );
}

#[sqlx::test(migrator = "crate::MIGRATOR")]
async fn users_are_listed_only_where_the_reader_has_scope(pool: PgPool) {
    let t = tenants(&pool).await;
    let a = upsert_user(&pool, "i", "a", Some("a"), None, None)
        .await
        .expect("user");
    let b = upsert_user(&pool, "i", "b", Some("b"), None, None)
        .await
        .expect("user");
    let c = upsert_user(&pool, "i", "c", Some("c"), None, None)
        .await
        .expect("user");
    insert_assignment(
        &pool,
        &assignment(
            PrincipalRef::User { id: a.id },
            Builtin::Viewer,
            Scope::Project { id: t.acme_prod.id },
        ),
    )
    .await
    .expect("assign");
    insert_assignment(
        &pool,
        &assignment(
            PrincipalRef::User { id: b.id },
            Builtin::Viewer,
            Scope::Project {
                id: t.other_proj.id,
            },
        ),
    )
    .await
    .expect("assign");
    let acme = AccessFilter::Limited {
        organizations: vec![t.acme.id],
        projects: vec![],
        applications: vec![],
    };
    let names = |v: Vec<UserRow>| v.into_iter().filter_map(|u| u.username).collect::<Vec<_>>();
    assert_eq!(
        names(list_users(&pool, &acme).await.expect("users")),
        vec!["a"],
        "b is in another organisation, c has no access anywhere"
    );
    assert_eq!(
        list_users(&pool, &AccessFilter::All)
            .await
            .expect("users")
            .len(),
        3
    );
    let _ = c;
}

#[sqlx::test(migrator = "crate::MIGRATOR")]
async fn the_database_refuses_inconsistent_assignments(pool: PgPool) {
    let t = tenants(&pool).await;
    let me = PrincipalRef::User {
        id: uuid::Uuid::new_v4(),
    };
    // An emergency grant must expire, carry a reason and sit at the platform level.
    let mut e = assignment(me, Builtin::FabricAdministrator, Scope::Platform);
    e.kind = AssignmentKind::Emergency;
    assert!(
        insert_assignment(&pool, &e).await.is_err(),
        "no expiry or reason"
    );
    e.expires_at = Some(OffsetDateTime::now_utc() + Duration::hours(1));
    e.reason = Some("quorum loss".into());
    insert_assignment(&pool, &e)
        .await
        .expect("valid emergency grant");
    let mut bad_scope = e.clone();
    bad_scope.scope = Scope::Organization { id: t.acme.id };
    assert!(
        insert_assignment(&pool, &bad_scope).await.is_err(),
        "emergency grants are platform-level"
    );
    // Exactly one of builtin and custom role.
    let r = sqlx::query("INSERT INTO role_assignments (id, principal_type, principal_id, scope_type, created_by) VALUES ($1,'user',$2,'platform','x')").bind(uuid::Uuid::new_v4()).bind(uuid::Uuid::new_v4()).execute(&pool).await;
    assert!(r.is_err(), "a role is required");
    // A scope type must come with exactly its own column.
    let r = sqlx::query("INSERT INTO role_assignments (id, principal_type, principal_id, builtin_role, scope_type, project_id, organization_id, created_by) VALUES ($1,'user',$2,'viewer','project',$3,$4,'x')")
        .bind(uuid::Uuid::new_v4()).bind(uuid::Uuid::new_v4()).bind(t.acme_prod.id).bind(t.acme.id).execute(&pool).await;
    assert!(r.is_err(), "two scope columns at once");
}

#[sqlx::test(migrator = "crate::MIGRATOR")]
async fn custom_roles_in_use_cannot_be_deleted(pool: PgPool) {
    let perms: BTreeSet<String> = ["application.view".to_string()].into();
    let role = create_custom_role(&pool, "Read only", "", &perms, "test")
        .await
        .expect("role");
    assert!(matches!(
        create_custom_role(&pool, "Read only", "", &perms, "test").await,
        Err(StoreError::Conflict(_))
    ));
    let me = PrincipalRef::User {
        id: uuid::Uuid::new_v4(),
    };
    let mut a = assignment(me, Builtin::Viewer, Scope::Platform);
    a.role = RoleRef::Custom { id: role.id };
    insert_assignment(&pool, &a).await.expect("assign");
    assert!(matches!(
        delete_custom_role(&pool, role.id).await,
        Err(StoreError::Conflict(_))
    ));
    let engine = load_engine(&pool, &[me]).await.expect("engine");
    assert_eq!(
        engine
            .custom_roles
            .get(&role.id)
            .map(|r| r.permissions.clone()),
        Some(perms),
        "custom roles load with the assignment"
    );
}

#[sqlx::test(migrator = "crate::MIGRATOR")]
async fn policies_feed_the_engine(pool: PgPool) {
    save_policy(
        &pool,
        "Production needs MFA",
        "mfa",
        &json!({ "environments": ["production"], "infrastructure": true }),
        true,
        "test",
    )
    .await
    .expect("policy");
    save_policy(
        &pool,
        "Deleting applications",
        "approval",
        &json!({ "actions": ["application.delete"] }),
        true,
        "test",
    )
    .await
    .expect("policy");
    save_policy(
        &pool,
        "Disabled",
        "mfa",
        &json!({ "environments": ["staging"] }),
        false,
        "test",
    )
    .await
    .expect("policy");
    let p = load_policies(&pool).await.expect("policies");
    assert_eq!(p.mfa_for_environments, vec![EnvKind::Production]);
    assert!(p.mfa_for_infrastructure && p.extra_approval.contains("application.delete"));
    let again = save_policy(
        &pool,
        "Production needs MFA",
        "mfa",
        &json!({ "environments": [] }),
        true,
        "test",
    )
    .await
    .expect("update in place");
    assert_eq!(list_policies(&pool).await.expect("list").len(), 3);
    assert_eq!(again.parameters, json!({ "environments": [] }));
}

#[sqlx::test(migrator = "crate::MIGRATOR")]
async fn only_automation_identities_have_tokens_and_misuse_is_refused(pool: PgPool) {
    let (ci, token) =
        create_service_identity(&pool, "ci", "automation", None, &json!({}), None, "test")
            .await
            .expect("service");
    let token = token.expect("automation identities get a token");
    assert!(token.starts_with("aps_"));
    assert_eq!(
        authenticate_service(&pool, &token)
            .await
            .expect("auth")
            .map(|s| s.id),
        Some(ci.id)
    );
    assert!(
        authenticate_service(&pool, "aps_wrong")
            .await
            .expect("auth")
            .is_none()
    );

    let (incus, none) = create_service_identity(
        &pool,
        "incus-deploy",
        "workload",
        Some("incus-deploy-cert"),
        &json!({ "projects": ["acme-prod"] }),
        None,
        "test",
    )
    .await
    .expect("service");
    assert!(none.is_none(), "outbound credentials get no API token");
    assert_eq!(incus.credential_ref.as_deref(), Some("incus-deploy-cert"));
    let stored: Option<Vec<u8>> =
        sqlx::query_scalar("SELECT token_hash FROM service_identities WHERE id = $1")
            .bind(incus.id)
            .fetch_one(&pool)
            .await
            .expect("hash");
    assert!(stored.is_none());
    let forced = sqlx::query("UPDATE service_identities SET token_hash = '\\x00' WHERE id = $1")
        .bind(incus.id)
        .execute(&pool)
        .await;
    assert!(
        forced.is_err(),
        "the database refuses a token on a workload identity"
    );

    set_service_identity_disabled(&pool, ci.id, true)
        .await
        .expect("disable");
    assert!(
        authenticate_service(&pool, &token)
            .await
            .expect("auth")
            .is_none(),
        "disabled identities stop working at once"
    );
    set_service_identity_disabled(&pool, ci.id, false)
        .await
        .expect("enable");
    let (_, exp) = create_service_identity(
        &pool,
        "old",
        "automation",
        None,
        &json!({}),
        Some(OffsetDateTime::now_utc() - Duration::hours(1)),
        "test",
    )
    .await
    .expect("service");
    assert!(
        authenticate_service(&pool, &exp.expect("token"))
            .await
            .expect("auth")
            .is_none(),
        "expired"
    );
}

#[sqlx::test(migrator = "crate::MIGRATOR")]
async fn approvals_follow_the_four_eyes_rule_and_are_single_use(pool: PgPool) {
    let a = request_approval(
        &pool,
        "hardware.delete",
        "machine:rack1-07",
        "user:a",
        "decommissioned",
    )
    .await
    .expect("request");
    assert_eq!(a.state, "pending");
    assert_eq!(
        decide_approval(&pool, a.id, "user:a", true, true)
            .await
            .expect("db"),
        Err(apelora_auth::ApprovalError::SameAsRequester)
    );
    assert_eq!(
        decide_approval(&pool, a.id, "user:b", false, true)
            .await
            .expect("db"),
        Err(apelora_auth::ApprovalError::ApproverNotPermitted)
    );
    let ok = decide_approval(&pool, a.id, "user:b", true, true)
        .await
        .expect("db")
        .expect("approved");
    assert_eq!(
        (ok.state.as_str(), ok.approver.as_deref()),
        ("approved", Some("user:b"))
    );
    assert_eq!(
        consume_approval(&pool, a.id, "hardware.delete", "machine:rack1-08", "user:a")
            .await
            .expect("db"),
        Err(apelora_auth::ApprovalError::Mismatch)
    );
    assert_eq!(
        consume_approval(&pool, a.id, "hardware.delete", "machine:rack1-07", "user:c")
            .await
            .expect("db"),
        Err(apelora_auth::ApprovalError::RequesterOnly)
    );
    // Two concurrent attempts to spend it: exactly one wins.
    let (p1, p2) = (pool.clone(), pool.clone());
    let id = a.id;
    let r1 = tokio::spawn(async move {
        consume_approval(&p1, id, "hardware.delete", "machine:rack1-07", "user:a")
            .await
            .expect("db")
            .is_ok()
    });
    let r2 = tokio::spawn(async move {
        consume_approval(&p2, id, "hardware.delete", "machine:rack1-07", "user:a")
            .await
            .expect("db")
            .is_ok()
    });
    let wins = [r1.await.expect("join"), r2.await.expect("join")]
        .into_iter()
        .filter(|w| *w)
        .count();
    assert_eq!(wins, 1);
    assert_eq!(
        list_approvals(&pool, None, true).await.expect("list").len(),
        0,
        "consumed approvals are not open"
    );
}

#[sqlx::test(migrator = "crate::MIGRATOR")]
async fn the_audit_trail_is_append_only_and_filtered_by_scope(pool: PgPool) {
    let t = tenants(&pool).await;
    let acme_res = Resource::project(t.acme.id, t.acme_prod.id);
    let other_res = Resource::project(t.other.id, t.other_proj.id);
    for (res, action, outcome) in [
        (Some(&acme_res), "application.deploy", "success"),
        (Some(&other_res), "application.deploy", "denied"),
        (None, "token.create", "success"),
    ] {
        audit_event(
            &pool,
            &AuditEntry {
                actor: "user:a",
                actor_type: "user",
                action,
                target: "x",
                outcome,
                scope: res,
                detail: &json!({}),
            },
        )
        .await
        .expect("audit");
    }
    let acme = AccessFilter::Limited {
        organizations: vec![t.acme.id],
        projects: vec![],
        applications: vec![],
    };
    let rows = audit_history(&pool, &acme, None, None, None, 50)
        .await
        .expect("history");
    assert_eq!(
        rows.len(),
        1,
        "only events inside the reader's scope; scope-less platform events are for platform readers"
    );
    assert_eq!(
        audit_history(&pool, &AccessFilter::All, None, None, None, 50)
            .await
            .expect("history")
            .len(),
        3
    );
    assert_eq!(
        audit_history(&pool, &AccessFilter::All, None, Some("denied"), None, 50)
            .await
            .expect("history")
            .len(),
        1
    );
    assert_eq!(
        audit_history(&pool, &AccessFilter::All, Some("token."), None, None, 50)
            .await
            .expect("history")
            .len(),
        1
    );
    assert!(
        sqlx::query("UPDATE audit_events SET actor = 'someone-else'")
            .execute(&pool)
            .await
            .is_err(),
        "rows cannot be changed"
    );
    assert!(
        sqlx::query("DELETE FROM audit_events")
            .execute(&pool)
            .await
            .is_err(),
        "rows cannot be deleted"
    );
}

#[sqlx::test(migrator = "crate::MIGRATOR")]
async fn the_last_super_admin_is_counted(pool: PgPool) {
    assert_eq!(super_admin_count(&pool).await.expect("count"), 0);
    let me = PrincipalRef::User {
        id: uuid::Uuid::new_v4(),
    };
    let id = insert_assignment(
        &pool,
        &assignment(me, Builtin::PlatformSuperAdmin, Scope::Platform),
    )
    .await
    .expect("assign");
    assert_eq!(super_admin_count(&pool).await.expect("count"), 1);
    revoke_assignment(&pool, id, "t").await.expect("revoke");
    assert_eq!(super_admin_count(&pool).await.expect("count"), 0);
}

#[sqlx::test(migrator = "crate::MIGRATOR")]
async fn the_last_enabled_super_admin_is_kept_whichever_way_they_would_go(pool: PgPool) {
    let person = |s: &'static str| {
        let pool = pool.clone();
        async move {
            upsert_user(&pool, "https://id.test", s, Some(s), None, None)
                .await
                .expect("user")
                .id
        }
    };
    let (ann, ben, cat) = (
        person("ann").await,
        person("ben").await,
        person("cat").await,
    );
    assert_eq!(enabled_super_admins(&pool).await.expect("count"), 0);
    // With no super admin at all, nothing is refused (a platform run on API tokens).
    assert!(
        change_keeping_a_super_admin(&pool, SuperAdminChange::DisableUser(cat))
            .await
            .expect("change")
    );
    set_user_disabled(&pool, cat, false).await.expect("enable");

    let direct = insert_assignment(
        &pool,
        &assignment(
            PrincipalRef::User { id: ann },
            Builtin::PlatformSuperAdmin,
            Scope::Platform,
        ),
    )
    .await
    .expect("assign");
    let g = create_group(&pool, None, "owners", None, "")
        .await
        .expect("group");
    let via_group = insert_assignment(
        &pool,
        &assignment(
            PrincipalRef::Group { id: g.id },
            Builtin::PlatformSuperAdmin,
            Scope::Platform,
        ),
    )
    .await
    .expect("assign");
    add_group_member(&pool, g.id, ben, "t")
        .await
        .expect("member");
    // Neither an emergency grant nor an expired one counts.
    let mut emergency = assignment(
        PrincipalRef::User { id: cat },
        Builtin::PlatformSuperAdmin,
        Scope::Platform,
    );
    emergency.kind = AssignmentKind::Emergency;
    emergency.expires_at = Some(OffsetDateTime::now_utc() + Duration::hours(1));
    emergency.reason = Some("incident".into());
    insert_assignment(&pool, &emergency).await.expect("assign");
    let mut expired = assignment(
        PrincipalRef::User { id: cat },
        Builtin::PlatformSuperAdmin,
        Scope::Platform,
    );
    expired.expires_at = Some(OffsetDateTime::now_utc() - Duration::minutes(1));
    insert_assignment(&pool, &expired).await.expect("assign");
    assert_eq!(enabled_super_admins(&pool).await.expect("count"), 2);

    // ann goes; ben, through the group, is the last one.
    assert!(
        change_keeping_a_super_admin(&pool, SuperAdminChange::DisableUser(ann))
            .await
            .expect("change")
    );
    assert_eq!(enabled_super_admins(&pool).await.expect("count"), 1);
    for change in [
        SuperAdminChange::DisableUser(ben),
        SuperAdminChange::RemoveGroupMember {
            group: g.id,
            user: ben,
        },
        SuperAdminChange::RevokeAssignment {
            id: via_group,
            by: "t",
        },
    ] {
        assert!(
            !change_keeping_a_super_admin(&pool, change)
                .await
                .expect("change"),
            "{change:?}"
        );
    }
    // Refused changes left nothing behind.
    assert_eq!(enabled_super_admins(&pool).await.expect("count"), 1);
    assert!(
        user(&pool, ben)
            .await
            .expect("user")
            .expect("ben")
            .disabled_at
            .is_none()
    );
    // The disabled person's own grant can go: it changes nothing.
    assert!(
        change_keeping_a_super_admin(
            &pool,
            SuperAdminChange::RevokeAssignment {
                id: direct,
                by: "t"
            }
        )
        .await
        .expect("change")
    );
    // Unknown targets are reported, not ignored.
    for change in [
        SuperAdminChange::DisableUser(uuid::Uuid::new_v4()),
        SuperAdminChange::RevokeAssignment {
            id: direct,
            by: "t",
        },
    ] {
        assert!(matches!(
            change_keeping_a_super_admin(&pool, change).await,
            Err(StoreError::NotFound(_))
        ));
    }
    // Removing someone who is not a member is not refused.
    assert!(
        change_keeping_a_super_admin(
            &pool,
            SuperAdminChange::RemoveGroupMember {
                group: g.id,
                user: cat
            }
        )
        .await
        .expect("change")
    );
}

#[sqlx::test(migrator = "crate::MIGRATOR")]
async fn concurrent_changes_never_leave_no_super_admin(pool: PgPool) {
    let mut people = Vec::new();
    for s in ["a", "b", "c", "d"] {
        let id = upsert_user(&pool, "https://id.test", s, Some(s), None, None)
            .await
            .expect("user")
            .id;
        insert_assignment(
            &pool,
            &assignment(
                PrincipalRef::User { id },
                Builtin::PlatformSuperAdmin,
                Scope::Platform,
            ),
        )
        .await
        .expect("assign");
        people.push(id);
    }
    let tasks: Vec<_> = people
        .iter()
        .map(|id| {
            let pool = pool.clone();
            let id = *id;
            tokio::spawn(async move {
                change_keeping_a_super_admin(&pool, SuperAdminChange::DisableUser(id)).await
            })
        })
        .collect();
    let mut done = 0;
    for t in tasks {
        if t.await.expect("task").expect("change") {
            done += 1;
        }
    }
    assert_eq!(done, 3, "all but one");
    assert_eq!(enabled_super_admins(&pool).await.expect("count"), 1);
}
