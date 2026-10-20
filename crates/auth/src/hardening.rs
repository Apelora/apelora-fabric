// SPDX-License-Identifier: AGPL-3.0-only
//! Tests written from mutation testing (`cargo mutants`): each pins a rule that the earlier
//! tests left unprotected, such as a boundary, a policy flag or a message people read.

use std::collections::BTreeSet;

use uuid::Uuid;

use crate::approval::{APPROVAL_TTL_SECS, ApprovalError, REQUEST_TTL_SECS};
use crate::assign::AssignError;
use crate::backend::incus_project_name;
use crate::evaluate::fixtures::{NOW, assign, builtin, engine, id, user, world};
use crate::evaluate::{AccessFilter, Conditions, Engine, PrincipalRef, Subject};
use crate::role::{Builtin, RoleError, validate_custom};
use crate::scope::{EnvKind, Resource, Scope};
use crate::service::ServiceKind;

#[test]
fn approval_lifetimes_are_a_day_to_decide_and_half_an_hour_to_use() {
    assert_eq!(REQUEST_TTL_SECS, 24 * 3600);
    assert_eq!(APPROVAL_TTL_SECS, 30 * 60);
}

#[test]
fn people_read_these_messages_so_none_may_be_blank() {
    for e in [
        ApprovalError::NotPending,
        ApprovalError::NotApproved,
        ApprovalError::Expired,
        ApprovalError::SameAsRequester,
        ApprovalError::ApproverNotPermitted,
        ApprovalError::RequesterOnly,
        ApprovalError::Mismatch,
    ] {
        assert!(e.to_string().len() > 10, "{e:?}");
    }
    for e in [
        AssignError::RoleNotAssignableAtScope,
        AssignError::NotPermitted,
        AssignError::WouldEscalate(vec!["x".into()]),
        AssignError::ForbiddenForService(vec!["x".into()]),
        AssignError::EmergencyRules("because"),
    ] {
        assert!(e.to_string().len() > 5, "{e:?}");
    }
    for e in [
        RoleError::NameInvalid,
        RoleError::NameTaken,
        RoleError::NoPermissions,
        RoleError::TooManyPermissions,
        RoleError::UnknownPermission("x".into()),
    ] {
        assert!(e.to_string().len() > 5, "{e:?}");
    }
    for e in [
        crate::backend::MaasError::UnsupportedVersion("3.5".into()),
        crate::backend::MaasError::NotRepresentable("x".into()),
    ] {
        assert!(e.to_string().len() > 20, "{e:?}");
    }
    for b in Builtin::ALL {
        assert!(b.description().len() > 20, "{b:?}");
    }
    for k in ServiceKind::ALL {
        assert!(!k.backend().is_empty());
    }
    assert_eq!(ServiceKind::Workload.backend(), "incus");
    assert_eq!(ServiceKind::Automation.backend(), "apelora");
}

#[test]
fn environment_kinds_round_trip_by_name() {
    for k in [
        EnvKind::Production,
        EnvKind::Staging,
        EnvKind::Development,
        EnvKind::Other,
    ] {
        assert_eq!(EnvKind::parse(k.as_str()), Some(k));
    }
    assert_eq!(EnvKind::Production.as_str(), "production");
    assert_eq!(EnvKind::parse("prod"), None);
    assert_eq!(EnvKind::parse(""), None);
}

#[test]
fn custom_role_definitions_are_checked_at_their_limits() {
    let perms = |n: usize| (0..n).map(|i| format!("fake.{i}")).collect::<BTreeSet<_>>();
    let ok = BTreeSet::from(["application.view".to_owned()]);
    assert_eq!(validate_custom("Ops team_1-a", &ok), Ok(()));
    assert_eq!(validate_custom(&"x".repeat(60), &ok), Ok(()));
    assert_eq!(
        validate_custom(&"x".repeat(61), &ok),
        Err(RoleError::NameInvalid)
    );
    assert_eq!(validate_custom("", &ok), Err(RoleError::NameInvalid));
    assert_eq!(validate_custom("   ", &ok), Err(RoleError::NameInvalid));
    assert_eq!(validate_custom("bad!", &ok), Err(RoleError::NameInvalid));
    assert_eq!(validate_custom("Viewer", &ok), Err(RoleError::NameTaken));
    assert_eq!(
        validate_custom("viewer", &ok),
        Err(RoleError::NameTaken),
        "case does not make a new name"
    );
    assert_eq!(
        validate_custom("platform-super-admin", &ok),
        Err(RoleError::NameTaken),
        "a role key is as taken as its display name"
    );
    assert_eq!(
        validate_custom("Ops", &BTreeSet::new()),
        Err(RoleError::NoPermissions)
    );
    assert!(matches!(
        validate_custom("Ops", &perms(100)),
        Err(RoleError::UnknownPermission(_))
    ));
    assert_eq!(
        validate_custom("Ops", &perms(101)),
        Err(RoleError::TooManyPermissions)
    );
}

#[test]
fn incus_project_names_are_cleaned_but_never_merge_parts() {
    assert_eq!(
        incus_project_name("Acme Corp", "Web Shop", "Live"),
        Some("acme-corp-web-shop-live".into())
    );
    assert_eq!(
        incus_project_name("a--b", "-c", "d-"),
        Some("a-b-c-d".into()),
        "runs of separators collapse, edges are trimmed"
    );
    assert_eq!(incus_project_name("a", "!!", "c"), None);
    assert_eq!(
        incus_project_name("1a", "b", "c"),
        None,
        "must start with a letter"
    );
    assert!(
        incus_project_name(&"a".repeat(30), &"b".repeat(30), "c").is_some(),
        "63 characters is the limit"
    );
    assert_eq!(
        incus_project_name(&"a".repeat(30), &"b".repeat(31), "c"),
        None
    );
}

fn service(n: u128) -> Subject {
    Subject {
        service: Some(id(n)),
        ..Subject::default()
    }
}

#[test]
fn a_service_with_an_ordinary_permission_is_listed_and_one_with_a_withheld_permission_is_not() {
    let w = world();
    let e = engine(vec![assign(
        1,
        PrincipalRef::Service { id: id(90) },
        Builtin::ApplicationOperator,
        Scope::Project { id: w.acme_prod },
    )]);
    let svc = service(90);
    assert_eq!(
        e.filter(&svc, "application.operate", NOW),
        AccessFilter::Limited {
            organizations: vec![],
            projects: vec![w.acme_prod],
            applications: vec![]
        }
    );
    assert!(e.holds_anywhere(&svc, "application.operate", NOW));
    // Another service holds nothing through this assignment.
    assert!(!e.holds_anywhere(&service(91), "application.operate", NOW));
    assert!(
        e.filter(&service(91), "application.operate", NOW)
            .is_empty()
    );
}

#[test]
fn a_subject_with_only_groups_is_not_mistaken_for_a_service() {
    let w = world();
    let group = id(70);
    let e = engine(vec![assign(
        1,
        PrincipalRef::Group { id: group },
        Builtin::OrganizationOwner,
        Scope::Organization { id: w.acme },
    )]);
    let s = Subject {
        groups: BTreeSet::from([group]),
        ..Subject::default()
    };
    assert!(e.holds_anywhere(&s, "role.assign", NOW));
    assert!(!e.filter(&s, "role.assign", NOW).is_empty());
    // A service with the same group claim never gets a person-only permission.
    let svc = Subject {
        service: Some(id(90)),
        groups: BTreeSet::from([group]),
        ..Subject::default()
    };
    assert!(!e.holds_anywhere(&svc, "role.assign", NOW));
    assert!(e.filter(&svc, "role.assign", NOW).is_empty());
}

#[test]
fn the_filter_honours_mfa_and_environment_conditions_like_the_check_does() {
    let w = world();
    let with = |c: Conditions| {
        let mut a = assign(
            1,
            PrincipalRef::User { id: id(1) },
            Builtin::ApplicationOperator,
            Scope::Project { id: w.acme_prod },
        );
        a.conditions = c;
        engine(vec![a])
    };
    let mfa = Conditions {
        require_mfa: true,
        ..Conditions::default()
    };
    let signed_in_with_mfa = Subject {
        mfa: true,
        ..user(1)
    };
    let e = with(mfa);
    assert!(e.filter(&user(1), "application.deploy", NOW).is_empty());
    assert!(
        !e.filter(&signed_in_with_mfa, "application.deploy", NOW)
            .is_empty()
    );
    // An environment condition cannot be judged without a resource, so the listing leaves it out.
    let e = with(Conditions {
        environments: Some(vec![EnvKind::Production]),
        ..Conditions::default()
    });
    assert!(
        e.filter(&signed_in_with_mfa, "application.deploy", NOW)
            .is_empty()
    );
    // Platform policy: infrastructure permissions can require MFA for everyone.
    let mut e = engine(vec![assign(
        1,
        PrincipalRef::User { id: id(1) },
        Builtin::FabricAdministrator,
        Scope::Platform,
    )]);
    e.policies.mfa_for_infrastructure = true;
    assert!(e.filter(&user(1), "cluster.manage", NOW).is_empty());
    assert_eq!(
        e.filter(&signed_in_with_mfa, "cluster.manage", NOW),
        AccessFilter::All
    );
}

#[test]
fn expired_revoked_and_foreign_assignments_never_open_the_route_gate() {
    let w = world();
    let mk = |n: u128, who: u128, expires: Option<i64>, revoked: bool| {
        let mut a = assign(
            n,
            PrincipalRef::User { id: id(who) },
            Builtin::ApplicationOperator,
            Scope::Project { id: w.acme_prod },
        );
        a.expires_at = expires;
        a.revoked = revoked;
        a
    };
    let perm = "application.operate";
    assert!(!engine(vec![mk(1, 1, Some(NOW - 1), false)]).holds_anywhere(&user(1), perm, NOW));
    assert!(!engine(vec![mk(1, 1, Some(NOW), false)]).holds_anywhere(&user(1), perm, NOW));
    assert!(!engine(vec![mk(1, 1, None, true)]).holds_anywhere(&user(1), perm, NOW));
    assert!(!engine(vec![mk(1, 2, None, false)]).holds_anywhere(&user(1), perm, NOW));
    assert!(engine(vec![mk(1, 1, Some(NOW + 1), false)]).holds_anywhere(&user(1), perm, NOW));
    // A role that does not contain the permission does not open it.
    assert!(!engine(vec![mk(1, 1, None, false)]).holds_anywhere(
        &user(1),
        "application.design",
        NOW
    ));
}

#[test]
fn an_empty_listing_says_so_and_a_full_one_does_not() {
    assert!(!AccessFilter::All.is_empty());
    let none = AccessFilter::Limited {
        organizations: vec![],
        projects: vec![],
        applications: vec![],
    };
    assert!(none.is_empty());
    for l in [
        AccessFilter::Limited {
            organizations: vec![id(1)],
            projects: vec![],
            applications: vec![],
        },
        AccessFilter::Limited {
            organizations: vec![],
            projects: vec![id(1)],
            applications: vec![],
        },
        AccessFilter::Limited {
            organizations: vec![],
            projects: vec![],
            applications: vec![id(1)],
        },
    ] {
        assert!(!l.is_empty());
    }
}

#[test]
fn a_service_assignment_does_not_match_another_service_or_a_person() {
    let s = service(1);
    assert_eq!(s.matches(&PrincipalRef::Service { id: id(1) }), Some(None));
    assert_eq!(s.matches(&PrincipalRef::Service { id: id(2) }), None);
    assert_eq!(s.matches(&PrincipalRef::User { id: id(1) }), None);
    let u = user(1);
    assert_eq!(u.matches(&PrincipalRef::User { id: id(2) }), None);
    assert_eq!(u.matches(&PrincipalRef::Service { id: id(1) }), None);
    let _ = (
        builtin(Builtin::Viewer),
        Uuid::nil(),
        Resource::platform(),
        Engine::default(),
    );
}
