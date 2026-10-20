use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use rcgen::{KeyPair, PKCS_ECDSA_P256_SHA256};
use serde_json::{Value, json};

use super::*;

const ISS: &str = "https://id.example.test/realms/apelora";
const AUD: &str = "apelora-api";
const NOW: i64 = 1_800_000_000;

struct Signer {
    pem: String,
    jwks: JwkSet,
    kid: String,
}

fn signer(kid: &str) -> Signer {
    let kp = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("key");
    let raw = kp.public_key_raw();
    assert_eq!(raw.len(), 65, "uncompressed P-256 point");
    let jwk = json!({ "kty": "EC", "crv": "P-256", "kid": kid, "use": "sig", "alg": "ES256", "x": B64.encode(&raw[1..33]), "y": B64.encode(&raw[33..65]) });
    Signer {
        pem: kp.serialize_pem(),
        jwks: serde_json::from_value(json!({ "keys": [jwk] })).expect("jwks"),
        kid: kid.into(),
    }
}

fn claims() -> Value {
    json!({ "iss": ISS, "aud": AUD, "sub": "user-123", "iat": NOW - 10, "exp": NOW + 300, "preferred_username": "alice", "email": "alice@example.test", "name": "Alice", "groups": ["/acme/operators", "platform"], "amr": ["pwd", "otp"] })
}

fn sign(s: &Signer, claims: &Value) -> String {
    let mut h = Header::new(Algorithm::ES256);
    h.kid = Some(s.kid.clone());
    encode(
        &h,
        claims,
        &EncodingKey::from_ec_pem(s.pem.as_bytes()).expect("pem"),
    )
    .expect("sign")
}

fn verifier(s: &Signer) -> Arc<Verifier> {
    Verifier::with_keys(OidcConfig::new(ISS, AUD), s.jwks.clone())
}

#[tokio::test]
async fn a_valid_token_yields_the_identity_with_groups_and_mfa() {
    let s = signer("k1");
    let id = verifier(&s)
        .verify(&sign(&s, &claims()), NOW)
        .await
        .expect("valid");
    assert_eq!((id.issuer.as_str(), id.subject.as_str()), (ISS, "user-123"));
    assert_eq!(id.username.as_deref(), Some("alice"));
    assert_eq!(id.email.as_deref(), Some("alice@example.test"));
    assert!(id.mfa);
    assert_eq!(
        id.groups.iter().map(String::as_str).collect::<Vec<_>>(),
        vec!["acme/operators", "platform"],
        "leading slash removed"
    );
    assert_eq!(id.expires_at, NOW + 300);
}

#[tokio::test]
async fn expired_not_yet_valid_and_overlong_tokens_are_refused() {
    let s = signer("k1");
    let v = verifier(&s);
    let mut c = claims();
    c["exp"] = json!(NOW - 31);
    assert_eq!(v.verify(&sign(&s, &c), NOW).await, Err(TokenError::Expired));
    c["exp"] = json!(NOW - 29);
    assert!(
        v.verify(&sign(&s, &c), NOW).await.is_ok(),
        "within the leeway"
    );
    let mut c = claims();
    c["nbf"] = json!(NOW + 600);
    assert_eq!(
        v.verify(&sign(&s, &c), NOW).await,
        Err(TokenError::NotYetValid)
    );
    let mut c = claims();
    c["iat"] = json!(NOW + 600);
    c["exp"] = json!(NOW + 900);
    assert_eq!(
        v.verify(&sign(&s, &c), NOW).await,
        Err(TokenError::NotYetValid)
    );
    let mut c = claims();
    c["exp"] = json!(NOW + 7200);
    assert_eq!(v.verify(&sign(&s, &c), NOW).await, Err(TokenError::TooLong));
}

#[tokio::test]
async fn wrong_issuer_or_audience_is_refused_including_other_applications_tokens() {
    let s = signer("k1");
    let v = verifier(&s);
    let mut c = claims();
    c["iss"] = json!("https://evil.example.test/realms/apelora");
    assert_eq!(
        v.verify(&sign(&s, &c), NOW).await,
        Err(TokenError::WrongIssuer)
    );
    let mut c = claims();
    c["aud"] = json!("portal.example.com");
    assert_eq!(
        v.verify(&sign(&s, &c), NOW).await,
        Err(TokenError::WrongAudience),
        "an application end-user token is not an Apelora token"
    );
    let mut c = claims();
    c["aud"] = json!(["account", AUD]);
    assert!(
        v.verify(&sign(&s, &c), NOW).await.is_ok(),
        "audience lists are accepted when one matches"
    );
    let mut c = claims();
    c.as_object_mut().expect("object").remove("aud");
    assert_eq!(
        v.verify(&sign(&s, &c), NOW).await,
        Err(TokenError::MissingClaim("aud"))
    );
}

#[tokio::test]
async fn missing_identity_claims_are_refused() {
    let s = signer("k1");
    let v = verifier(&s);
    for name in ["sub", "exp", "iat", "iss"] {
        let mut c = claims();
        c.as_object_mut().expect("object").remove(name);
        assert!(
            matches!(v.verify(&sign(&s, &c), NOW).await, Err(TokenError::MissingClaim(n)) if n == name),
            "{name}"
        );
    }
    let mut c = claims();
    c["sub"] = json!("");
    assert_eq!(
        v.verify(&sign(&s, &c), NOW).await,
        Err(TokenError::MissingClaim("sub"))
    );
}

#[tokio::test]
async fn forged_tampered_and_foreign_keys_are_refused() {
    let s = signer("k1");
    let other = signer("k1");
    let v = verifier(&s);
    assert_eq!(
        v.verify(&sign(&other, &claims()), NOW).await,
        Err(TokenError::BadSignature),
        "same kid, different key"
    );
    let foreign = signer("unpublished");
    assert_eq!(
        v.verify(&sign(&foreign, &claims()), NOW).await,
        Err(TokenError::UnknownKey)
    );
    // Tamper with the payload: change the subject, keep the signature.
    let good = sign(&s, &claims());
    let parts: Vec<&str> = good.split('.').collect();
    let mut c = claims();
    c["sub"] = json!("admin");
    let forged = format!(
        "{}.{}.{}",
        parts[0],
        B64.encode(serde_json::to_vec(&c).expect("json")),
        parts[2]
    );
    assert_eq!(v.verify(&forged, NOW).await, Err(TokenError::BadSignature));
}

#[tokio::test]
async fn the_none_algorithm_and_shared_secret_algorithms_are_refused() {
    let s = signer("k1");
    let v = verifier(&s);
    let none = format!(
        "{}.{}.",
        B64.encode(br#"{"alg":"none","typ":"JWT"}"#),
        B64.encode(serde_json::to_vec(&claims()).expect("json"))
    );
    assert!(matches!(
        v.verify(&none, NOW).await,
        Err(TokenError::Malformed | TokenError::UnsupportedAlgorithm)
    ));
    let hs = encode(
        &Header::new(Algorithm::HS256),
        &claims(),
        &EncodingKey::from_secret(b"secret"),
    )
    .expect("hs256");
    assert_eq!(
        v.verify(&hs, NOW).await,
        Err(TokenError::UnsupportedAlgorithm),
        "HS256 confusion attack"
    );
    assert_eq!(
        v.verify("not-a-token", NOW).await,
        Err(TokenError::Malformed)
    );
    assert_eq!(v.verify("a.b.c", NOW).await, Err(TokenError::Malformed));
}

#[tokio::test]
async fn authorised_parties_and_mfa_rules_follow_configuration() {
    let s = signer("k1");
    let mut cfg = OidcConfig::new(ISS, AUD);
    cfg.authorized_parties = vec!["apelora-console".into()];
    cfg.mfa_amr = vec![];
    cfg.mfa_acr = vec!["gold".into()];
    let v = Verifier::with_keys(cfg, s.jwks.clone());
    let mut c = claims();
    assert_eq!(
        v.verify(&sign(&s, &c), NOW).await,
        Err(TokenError::UnauthorizedParty),
        "no azp"
    );
    c["azp"] = json!("some-other-client");
    assert_eq!(
        v.verify(&sign(&s, &c), NOW).await,
        Err(TokenError::UnauthorizedParty)
    );
    c["azp"] = json!("apelora-console");
    assert!(
        !v.verify(&sign(&s, &c), NOW).await.expect("ok").mfa,
        "amr otp no longer counts"
    );
    c["acr"] = json!("gold");
    assert!(v.verify(&sign(&s, &c), NOW).await.expect("ok").mfa);
}

#[tokio::test]
async fn a_single_unlabelled_key_is_accepted_but_ambiguity_is_not() {
    let s = signer("k1");
    let mut h = Header::new(Algorithm::ES256);
    h.kid = None;
    let t = encode(
        &h,
        &claims(),
        &EncodingKey::from_ec_pem(s.pem.as_bytes()).expect("pem"),
    )
    .expect("sign");
    assert!(verifier(&s).verify(&t, NOW).await.is_ok());
    let b = signer("k2");
    let mut both = s.jwks.clone();
    both.keys.extend(b.jwks.keys.clone());
    let v = Verifier::with_keys(OidcConfig::new(ISS, AUD), both);
    assert_eq!(v.verify(&t, NOW).await, Err(TokenError::UnknownKey));
}

#[test]
fn remote_key_sets_need_tls() {
    let cfg = || OidcConfig::new(ISS, AUD);
    assert!(
        Verifier::with_remote_keys(
            cfg(),
            "http://keys.example.test/jwks",
            Duration::from_secs(30)
        )
        .is_err()
    );
    assert!(
        Verifier::with_remote_keys(
            cfg(),
            "https://keys.example.test/jwks",
            Duration::from_secs(30)
        )
        .is_ok()
    );
    assert!(
        Verifier::with_remote_keys(cfg(), "http://127.0.0.1:8081/jwks", Duration::from_secs(30))
            .is_ok(),
        "loopback is for local development"
    );
}

#[test]
fn tells_tokens_from_api_tokens() {
    assert!(looks_like_jwt("aaa.bbb.ccc"));
    assert!(!looks_like_jwt("apl_0123456789abcdef"));
    assert!(!looks_like_jwt("a.b"));
    assert!(!looks_like_jwt("a.b.c.d"));
    assert!(!looks_like_jwt("..x"));
}

#[test]
fn the_issuer_is_read_only_to_choose_a_verifier() {
    use base64::Engine as _;
    let enc = |v: &str| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v);
    let token = format!(
        "{}.{}.sig",
        enc("{\"alg\":\"RS256\"}"),
        enc("{\"iss\":\"https://id.example.test\",\"sub\":\"a\"}")
    );
    assert_eq!(
        super::unverified_issuer(&token).as_deref(),
        Some("https://id.example.test")
    );
    assert_eq!(super::unverified_issuer("not.a.token"), None);
    assert_eq!(super::unverified_issuer("apl_abc"), None);
}

#[test]
fn only_secure_or_local_urls_are_accepted() {
    assert!(super::secure_url("https://id.example.test"));
    assert!(super::secure_url("http://127.0.0.1:5556/dex"));
    assert!(!super::secure_url("http://id.example.test"));
    assert!(!super::secure_url("ftp://x"));
}
