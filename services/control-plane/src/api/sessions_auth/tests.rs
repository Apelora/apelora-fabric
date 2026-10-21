use super::*;

fn login(f: impl FnOnce(&mut LocalLogin)) -> LocalLogin {
    let mut l = LocalLogin {
        user_id: Uuid::nil(),
        username: "alice".into(),
        display_name: None,
        email: None,
        password_hash: Some("$argon2id$stored".into()),
        disabled: false,
        locked: false,
    };
    f(&mut l);
    l
}

#[test]
fn the_policy_is_length_and_not_the_username() {
    assert!(check_password("short", "alice").is_err());
    assert!(check_password(&"a".repeat(11), "alice").is_err());
    assert!(check_password(&"a".repeat(12), "alice").is_ok());
    assert!(
        check_password(&"é".repeat(12), "alice").is_ok(),
        "characters, not bytes"
    );
    assert!(check_password(&"a".repeat(128), "alice").is_ok());
    assert!(check_password(&"a".repeat(129), "alice").is_err());
    assert!(check_password("alice.example", "alice.example").is_err());
    assert!(
        check_password("ALICE.EXAMPLE", "alice.example").is_err(),
        "case does not matter"
    );
    assert!(
        check_password("correct horse battery", "alice").is_ok(),
        "no composition rules"
    );
}

#[test]
fn usernames_are_lowercased_and_limited() {
    assert_eq!(normalize_username(" Alice.B "), Some("alice.b".into()));
    assert_eq!(normalize_username("ops_team-2"), Some("ops_team-2".into()));
    assert_eq!(normalize_username("a"), None);
    assert_eq!(normalize_username(&"a".repeat(65)), None);
    assert_eq!(normalize_username("alice smith"), None);
    assert_eq!(normalize_username("alice@example.org"), None);
    assert_eq!(normalize_username("ålice"), None);
}

#[test]
fn only_a_live_account_with_a_password_is_compared_everything_else_verifies_the_dummy() {
    assert_eq!(plan(None), Plan::Refuse("unknown"));
    assert_eq!(
        plan(Some(&login(|l| l.disabled = true))),
        Plan::Refuse("disabled")
    );
    assert_eq!(
        plan(Some(&login(|l| l.locked = true))),
        Plan::Refuse("locked")
    );
    assert_eq!(
        plan(Some(&login(|l| l.password_hash = None))),
        Plan::Refuse("wrong"),
        "not set up yet"
    );
    assert_eq!(
        plan(Some(&login(|_| {}))),
        Plan::Verify("$argon2id$stored".into())
    );
}

#[tokio::test]
async fn hashes_are_argon2id_with_the_agreed_cost_and_verify() {
    let h = hash_password("correct horse battery".into())
        .await
        .expect("hash");
    assert!(h.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"), "{h}");
    assert!(verify_password("correct horse battery".into(), Some(h.clone())).await);
    assert!(!verify_password("correct horse batterY".into(), Some(h.clone())).await);
    assert!(
        !verify_password("x".repeat(10_000), Some(h)).await,
        "overlong input never matches"
    );
    let before = DUMMY_VERIFICATIONS.load(Ordering::Relaxed);
    let started = Instant::now();
    assert!(!verify_password("anything at all".into(), None).await);
    assert!(
        DUMMY_VERIFICATIONS.load(Ordering::Relaxed) > before,
        "the dummy hash was verified"
    );
    assert!(
        dummy_hash().starts_with("$argon2id$v=19$m=19456,t=2,p=1$"),
        "same cost as a real hash"
    );
    assert!(started.elapsed() > Duration::from_millis(1));
}

#[test]
fn ten_attempts_a_minute_per_address() {
    let a = Attempts::default();
    let ip: IpAddr = "192.0.2.7".parse().expect("ip");
    let other: IpAddr = "192.0.2.8".parse().expect("ip");
    let t0 = Instant::now();
    for _ in 0..10 {
        assert!(a.check(ip, t0).is_ok());
    }
    let wait = a
        .check(ip, t0 + Duration::from_secs(20))
        .expect_err("limited");
    assert_eq!(wait, 40);
    assert!(
        a.check(other, t0).is_ok(),
        "addresses are counted separately"
    );
    assert!(
        a.check(ip, t0 + Duration::from_secs(61)).is_ok(),
        "the window moves on"
    );
}
