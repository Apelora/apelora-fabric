use super::*;

fn fields() -> DirectoryFields {
    DirectoryFields {
        name: "Lab".into(),
        url: "ldaps://ldap.example.test".into(),
        bind_template: "uid={username},ou=people,dc=example,dc=test".into(),
        id_attribute: "entryUUID".into(),
        username_attribute: "uid".into(),
        email_attribute: "mail".into(),
        name_attribute: "cn".into(),
        group_attribute: Some("memberOf".into()),
        group_name_attribute: "cn".into(),
        enabled: true,
        ..DirectoryFields::default()
    }
}

#[test]
fn dn_values_are_escaped_per_rfc_4514() {
    assert_eq!(escape_dn_value("ada"), "ada");
    assert_eq!(escape_dn_value("ada,ou=admins"), "ada\\,ou\\=admins");
    assert_eq!(
        escape_dn_value("a+b;c<d>e\"f\\g"),
        "a\\+b\\;c\\<d\\>e\\\"f\\\\g"
    );
    assert_eq!(escape_dn_value("#lead"), "\\#lead");
    assert_eq!(escape_dn_value("mid#dle"), "mid#dle");
    assert_eq!(escape_dn_value(" both "), "\\ both\\ ");
    assert_eq!(escape_dn_value("in side"), "in side");
    assert_eq!(escape_dn_value("nul\0x"), "nul\\00x");
    assert_eq!(escape_dn_value("é"), "é", "UTF-8 needs no escaping");
}

#[test]
fn filter_values_are_escaped_per_rfc_4515() {
    assert_eq!(escape_filter_value("ada"), "ada");
    assert_eq!(escape_filter_value("*"), "\\2a");
    assert_eq!(escape_filter_value("*)(uid=*"), "\\2a\\29\\28uid=\\2a");
    assert_eq!(escape_filter_value("a\\b"), "a\\5cb");
    assert_eq!(escape_filter_value("nul\0"), "nul\\00");
    // The escaped value still parses as a filter with exactly one equality match.
    let filter = format!("(uid={})", escape_filter_value("*)(|(uid=*"));
    assert!(ldap3::parse_filter(&filter).is_ok(), "{filter}");
}

#[test]
fn injection_attempts_stay_inside_the_value() {
    let f = fields();
    // A comma cannot add an RDN that moves the bind to another branch.
    let name = bind_name(&f.bind_template, "ada,ou=admins");
    assert_eq!(name, "uid=ada\\,ou\\=admins,ou=people,dc=example,dc=test");
    assert_eq!(
        normalize_dn(&name).as_deref(),
        Some("uid=ada\\,ou\\=admins,ou=people,dc=example,dc=test"),
        "one RDN with the comma inside its value"
    );
    // A filter wildcard cannot match everyone.
    let filter = "(&(objectClass=person)(uid={username}))"
        .replace(USERNAME, &escape_filter_value("*)(uid=*"));
    assert_eq!(filter, "(&(objectClass=person)(uid=\\2a\\29\\28uid=\\2a))");
    // A principal name cannot point at another domain.
    assert_eq!(
        check_username("ada@other.test", BindForm::Principal),
        Err("type the username without a domain")
    );
    assert_eq!(check_username("ada@other.test", BindForm::Dn), Ok(()));
    assert!(check_username("ada\nx", BindForm::Dn).is_err());
    assert!(check_username("", BindForm::Dn).is_err());
    assert_eq!(
        bind_name("{username}@example.test", "ada"),
        "ada@example.test"
    );
}

#[tokio::test]
async fn an_empty_password_is_refused_before_anything_is_sent() {
    // Port 9 on loopback: nothing would answer, so reaching the network would fail differently.
    let mut f = fields();
    f.url = "ldaps://127.0.0.1:9".into();
    assert_eq!(
        sign_in(&f, "ada", "", Duration::from_secs(1)).await,
        Err(Failure::BindRefused)
    );
    assert_eq!(
        sign_in(&f, "", "secret", Duration::from_secs(1)).await,
        Err(Failure::BindRefused)
    );
}

#[tokio::test]
async fn plain_ldap_is_refused_without_connecting() {
    let mut f = fields();
    f.url = "ldap://127.0.0.1:9".into();
    let e = reach(&f, Duration::from_secs(1))
        .await
        .expect_err("refused");
    assert_eq!(e.problem(), DirectoryProblem::Settings);
    assert!(e.message().contains("plain LDAP is refused"));
}

#[test]
fn dns_have_one_spelling() {
    let ops = "cn=ops\\, europe,ou=groups,dc=example,dc=test";
    for spelling in [
        "cn=ops\\, europe,ou=groups,dc=example,dc=test",
        "CN=Ops\\, Europe,OU=Groups,DC=example,DC=test",
        "cn=Ops\\2C Europe, ou=groups , dc=example,dc=test",
        " cn = ops\\, europe ,ou=groups,dc=example,dc=test",
    ] {
        assert_eq!(normalize_dn(spelling).as_deref(), Some(ops), "{spelling}");
    }
    assert_eq!(
        normalize_dn("cn=a\\2cb,dc=x").as_deref(),
        Some("cn=a\\,b,dc=x")
    );
    // Multi-valued RDNs in any order.
    assert_eq!(
        normalize_dn("uid=x+cn=y,dc=t"),
        normalize_dn("CN=y+UID=x,dc=t")
    );
    for not_a_dn in ["plain-group-name", "cn=,dc=x", "=x", "c n=x"] {
        assert_eq!(normalize_dn(not_a_dn), None, "{not_a_dn}");
    }
}

#[test]
fn a_group_is_its_full_dn_not_its_first_rdn() {
    let mut e = entry();
    e.text.insert(
        "memberOf".into(),
        vec![
            "cn=platform-admins,ou=self-service,dc=example,dc=test".into(),
            "CN=Platform-Admins,OU=Groups,DC=example,DC=test".into(),
        ],
    );
    let p = person_from(&fields(), &e, "ada", &[]);
    assert_eq!(
        p.groups,
        vec![
            "cn=platform-admins,ou=groups,dc=example,dc=test",
            "cn=platform-admins,ou=self-service,dc=example,dc=test"
        ],
        "two groups of one name in two branches stay two groups"
    );
}

fn entry() -> Entry {
    let mut e = Entry {
        dn: "uid=ada,ou=people,dc=example,dc=test".into(),
        ..Entry::default()
    };
    let mut put = |k: &str, v: &[&str]| {
        e.text
            .insert(k.into(), v.iter().map(|s| (*s).to_owned()).collect());
    };
    put("entryuuid", &["0b6a7c1e-1111-4c1a-9f00-000000000001"]);
    put("UID", &["ada"]);
    put("mail", &["ada@example.test"]);
    put("cn", &["Ada Admin"]);
    put(
        "memberOf",
        &[
            "cn=dev-admins,ou=groups,dc=example,dc=test",
            "cn=dev-operators,ou=groups,dc=example,dc=test",
            "plain",
        ],
    );
    e
}

#[test]
fn attributes_and_groups_are_mapped_without_regard_to_case() {
    let p = person_from(
        &fields(),
        &entry(),
        "ADA",
        &[
            "cn=dev-operators,ou=groups,dc=example,dc=test".into(),
            "cn=searched,ou=groups,dc=example,dc=test".into(),
        ],
    );
    assert_eq!(p.subject, "0b6a7c1e-1111-4c1a-9f00-000000000001");
    assert_eq!(
        p.username, "ada",
        "the entry's spelling, not what was typed"
    );
    assert_eq!(p.email.as_deref(), Some("ada@example.test"));
    assert_eq!(p.display_name.as_deref(), Some("Ada Admin"));
    assert_eq!(
        p.groups,
        vec![
            "cn=dev-admins,ou=groups,dc=example,dc=test",
            "cn=dev-operators,ou=groups,dc=example,dc=test",
            "cn=searched,ou=groups,dc=example,dc=test",
            "plain"
        ]
    );
}

#[test]
fn the_subject_falls_back_to_a_guid_as_hex_then_to_the_dn() {
    let mut f = fields();
    f.id_attribute = "objectGUID".into();
    f.group_attribute = None;
    let mut e = entry();
    e.text.remove("entryuuid");
    e.binary
        .insert("objectGUID".into(), vec![vec![0x0a, 0xff, 0x01]]);
    let p = person_from(&f, &e, "ada", &[]);
    assert_eq!(p.subject, "0aff01");
    assert!(p.groups.is_empty(), "no group settings, no groups");
    e.binary.clear();
    assert_eq!(person_from(&f, &e, "ada", &[]).subject, e.dn);
    // Missing attributes: the typed username stays, the rest is unknown.
    let bare = Entry {
        dn: "uid=x,dc=t".into(),
        ..Entry::default()
    };
    let p = person_from(&f, &bare, "x", &[]);
    assert_eq!(
        (p.username.as_str(), p.email, p.display_name),
        ("x", None, None)
    );
}

#[test]
fn the_attributes_asked_for_have_no_duplicates() {
    let mut f = fields();
    f.id_attribute = "entryuuid".into();
    assert_eq!(
        person_attributes(&f),
        vec!["entryuuid", "objectGUID", "uid", "mail", "cn", "memberOf"]
    );
}

#[test]
fn a_ca_bundle_is_read_and_garbage_is_refused() {
    let ca = rcgen::generate_simple_self_signed(vec!["ca.test".into()]).expect("cert");
    let pem = ca.cert.pem();
    assert_eq!(parse_ca(&pem).expect("one").len(), 1);
    assert_eq!(parse_ca(&format!("{pem}\n{pem}")).expect("two").len(), 2);
    assert!(parse_ca("not a certificate").is_err());
    assert!(parse_ca("-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n").is_err());
}
