// SPDX-License-Identifier: AGPL-3.0-only
//! OIDC access-token validation for the control plane.
//!
//! The identity provider (Keycloak or any OpenID Connect provider) authenticates people. This
//! crate only decides whether a bearer token it issued is acceptable *to Apelora*: genuine
//! signature from a key in the provider's published key set, from the expected issuer, meant
//! for this API (audience), within its validity window, and carrying the claims Apelora needs.
//! What the person may then do is decided by `apelora-auth` from Apelora's own role assignments,
//! never from the token's claims alone.
//!
//! **Not yet validated against a running Keycloak.** The tests use locally generated keys and a
//! static key set; see `docs/architecture/IDENTITY-AND-AUTHORIZATION.md`.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use jsonwebtoken::jwk::{Jwk, JwkSet};
use jsonwebtoken::{Algorithm, DecodingKey, Header, Validation, decode, decode_header};
use serde_json::{Map, Value};
use tokio::sync::RwLock;

#[derive(Debug, Clone)]
pub struct OidcConfig {
    /// Exact `iss` value the provider puts in its tokens (for Keycloak, the realm URL).
    pub issuer: String,
    /// The API's own identifier(s); a token must name at least one in `aud`.
    pub audiences: Vec<String>,
    /// Accepted signature algorithms. Never `none`, never a shared-secret (HS*) algorithm.
    pub algorithms: Vec<Algorithm>,
    pub leeway_secs: i64,
    /// Name of the claim that lists the person's groups (a flat array of strings).
    pub groups_claim: String,
    pub username_claim: String,
    /// `amr` values that count as multi-factor authentication.
    pub mfa_amr: Vec<String>,
    /// `acr` values that count as multi-factor authentication.
    pub mfa_acr: Vec<String>,
    /// When non-empty, `azp` (the client the token was issued to) must be one of these.
    pub authorized_parties: Vec<String>,
    /// Refuse tokens whose lifetime (`exp` − `iat`) exceeds this many seconds.
    pub max_lifetime_secs: i64,
}

impl OidcConfig {
    pub fn new(issuer: impl Into<String>, audience: impl Into<String>) -> Self {
        Self {
            issuer: issuer.into(),
            audiences: vec![audience.into()],
            algorithms: vec![Algorithm::RS256, Algorithm::ES256],
            leeway_secs: 30,
            groups_claim: "groups".into(),
            username_claim: "preferred_username".into(),
            mfa_amr: vec!["mfa".into(), "otp".into(), "hwk".into()],
            mfa_acr: Vec::new(),
            authorized_parties: Vec::new(),
            max_lifetime_secs: 3600,
        }
    }
}

/// A person (or other subject) whose token has been accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub issuer: String,
    pub subject: String,
    pub username: Option<String>,
    pub email: Option<String>,
    pub name: Option<String>,
    /// Group names from the token, as the provider spelled them.
    pub groups: BTreeSet<String>,
    pub mfa: bool,
    pub expires_at: i64,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TokenError {
    #[error("the token is not a well-formed signed token")]
    Malformed,
    #[error("the token uses a signature algorithm Apelora does not accept")]
    UnsupportedAlgorithm,
    /// Names the token's key id (a public identifier, not key material).
    #[error("the token was signed with a key the identity provider does not publish (key id {})", .0.as_deref().unwrap_or("none"))]
    UnknownKey(Option<String>),
    #[error("the token signature is not valid")]
    BadSignature,
    #[error("the token has expired")]
    Expired,
    #[error("the token is not valid yet")]
    NotYetValid,
    /// Issuer and audience values are public identifiers: naming them shows a misconfiguration
    /// (e.g. a trailing slash, or the client id where the API's identifier was expected).
    #[error(
        "the token was issued by a different identity provider ({received}; expected {expected})"
    )]
    WrongIssuer { expected: String, received: String },
    #[error(
        "the token is not meant for this API (audience {received}; expected one of {expected})"
    )]
    WrongAudience { expected: String, received: String },
    #[error("the token was issued to a client that is not allowed")]
    UnauthorizedParty,
    #[error("the token lives longer than Apelora accepts")]
    TooLong,
    #[error("the token lacks the {0} claim")]
    MissingClaim(&'static str),
    #[error("the identity provider's keys cannot be read: {0}")]
    KeysUnavailable(String),
}

/// A value taken from an unverified token, made safe to show in a message or a log line:
/// control characters dropped, at most 120 characters.
fn shown(v: &str) -> String {
    let mut out: String = v.chars().filter(|c| !c.is_control()).take(120).collect();
    if v.chars().count() > 120 {
        out.push('…');
    }
    out
}

enum Keys {
    Static(JwkSet),
    Remote {
        url: String,
        client: reqwest::Client,
        cache: RwLock<Cache>,
        min_refresh: Duration,
    },
}

struct Cache {
    set: Option<JwkSet>,
    fetched: Option<Instant>,
}

pub struct Verifier {
    cfg: OidcConfig,
    keys: Keys,
}

impl Verifier {
    /// Uses a fixed key set (tests, or a provider whose keys are pinned in configuration).
    pub fn with_keys(cfg: OidcConfig, keys: JwkSet) -> Arc<Self> {
        Arc::new(Self {
            cfg,
            keys: Keys::Static(keys),
        })
    }

    /// Reads the provider's published key set (`jwks_uri`) over TLS and re-reads it when a token
    /// names an unknown key id, at most once per `min_refresh`.
    pub fn with_remote_keys(
        cfg: OidcConfig,
        jwks_url: &str,
        min_refresh: Duration,
    ) -> Result<Arc<Self>, TokenError> {
        if !secure_url(jwks_url) {
            return Err(TokenError::KeysUnavailable(
                "the key set URL must use https".into(),
            ));
        }
        let client = http_client()?;
        Ok(Arc::new(Self {
            cfg,
            keys: Keys::Remote {
                url: jwks_url.to_owned(),
                client,
                cache: RwLock::new(Cache {
                    set: None,
                    fetched: None,
                }),
                min_refresh,
            },
        }))
    }

    pub fn config(&self) -> &OidcConfig {
        &self.cfg
    }

    async fn key_for(&self, kid: Option<&str>, alg: Algorithm) -> Result<DecodingKey, TokenError> {
        let find = |set: &JwkSet| -> Option<Jwk> {
            match kid {
                Some(k) => set.find(k).cloned(),
                // Without a key id, accept only an unambiguous single key.
                None if set.keys.len() == 1 => set.keys.first().cloned(),
                None => None,
            }
        };
        let jwk = match &self.keys {
            Keys::Static(set) => find(set),
            Keys::Remote {
                url,
                client,
                cache,
                min_refresh,
            } => {
                let cached = cache.read().await.set.as_ref().and_then(find);
                if let Some(j) = cached {
                    Some(j)
                } else {
                    let mut c = cache.write().await;
                    let stale = c.fetched.is_none_or(|t| t.elapsed() >= *min_refresh);
                    if stale {
                        let fetched = async {
                            let res = client.get(url).send().await.map_err(|e| e.to_string())?;
                            successful(res)?
                                .json::<JwkSet>()
                                .await
                                .map_err(|e| e.to_string())
                        }
                        .await
                        .map_err(TokenError::KeysUnavailable);
                        apelora_metrics::IDP_KEY_FETCHES
                            .inc(&["key_set", if fetched.is_ok() { "ok" } else { "error" }]);
                        let fetched = fetched?;
                        c.set = Some(fetched);
                        c.fetched = Some(Instant::now());
                    }
                    c.set.as_ref().and_then(find)
                }
            }
        };
        let jwk = jwk.ok_or_else(|| TokenError::UnknownKey(kid.map(shown)))?;
        // The key must be meant for signatures and for this algorithm family.
        if let Some(a) = jwk.common.key_algorithm
            && format!("{a:?}") != format!("{alg:?}")
        {
            return Err(TokenError::UnsupportedAlgorithm);
        }
        DecodingKey::from_jwk(&jwk).map_err(|_| TokenError::UnknownKey(kid.map(shown)))
    }

    /// Validates the token at time `now` (seconds since the epoch) and returns who it is for.
    pub async fn verify(&self, token: &str, now: i64) -> Result<Identity, TokenError> {
        let header: Header = decode_header(token).map_err(|_| TokenError::Malformed)?;
        if !self.cfg.algorithms.contains(&header.alg) {
            return Err(TokenError::UnsupportedAlgorithm);
        }
        let key = self.key_for(header.kid.as_deref(), header.alg).await?;
        let mut v = Validation::new(header.alg);
        v.algorithms = vec![header.alg];
        // Time, issuer and audience are checked below against `now`, with precise errors.
        v.validate_exp = false;
        v.validate_nbf = false;
        v.validate_aud = false;
        v.required_spec_claims = BTreeSet::new().into_iter().collect();
        let data = decode::<Map<String, Value>>(token, &key, &v).map_err(|e| {
            use jsonwebtoken::errors::ErrorKind as K;
            match e.kind() {
                K::InvalidSignature | K::InvalidEcdsaKey | K::InvalidRsaKey(_) => {
                    TokenError::BadSignature
                }
                K::InvalidAlgorithm | K::InvalidAlgorithmName | K::MissingAlgorithm => {
                    TokenError::UnsupportedAlgorithm
                }
                _ => TokenError::Malformed,
            }
        })?;
        let c = data.claims;

        let issuer = c
            .get("iss")
            .and_then(Value::as_str)
            .ok_or(TokenError::MissingClaim("iss"))?;
        if issuer != self.cfg.issuer {
            return Err(TokenError::WrongIssuer {
                expected: self.cfg.issuer.clone(),
                received: shown(issuer),
            });
        }
        let aud_ok = match c.get("aud") {
            Some(Value::String(a)) => self.cfg.audiences.iter().any(|x| x == a),
            Some(Value::Array(list)) => list
                .iter()
                .filter_map(Value::as_str)
                .any(|a| self.cfg.audiences.iter().any(|x| x == a)),
            _ => return Err(TokenError::MissingClaim("aud")),
        };
        if !aud_ok {
            let received = match c.get("aud") {
                Some(Value::Array(list)) => list
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", "),
                Some(v) => v.as_str().unwrap_or_default().to_owned(),
                None => String::new(),
            };
            return Err(TokenError::WrongAudience {
                expected: self.cfg.audiences.join(", "),
                received: shown(&received),
            });
        }
        let exp = c
            .get("exp")
            .and_then(Value::as_i64)
            .ok_or(TokenError::MissingClaim("exp"))?;
        if exp + self.cfg.leeway_secs <= now {
            return Err(TokenError::Expired);
        }
        if let Some(nbf) = c.get("nbf").and_then(Value::as_i64)
            && nbf - self.cfg.leeway_secs > now
        {
            return Err(TokenError::NotYetValid);
        }
        let iat = c
            .get("iat")
            .and_then(Value::as_i64)
            .ok_or(TokenError::MissingClaim("iat"))?;
        if iat - self.cfg.leeway_secs > now {
            return Err(TokenError::NotYetValid);
        }
        if exp - iat > self.cfg.max_lifetime_secs {
            return Err(TokenError::TooLong);
        }
        let subject = c
            .get("sub")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty() && s.len() <= 255)
            .ok_or(TokenError::MissingClaim("sub"))?;
        if !self.cfg.authorized_parties.is_empty() {
            let azp = c.get("azp").and_then(Value::as_str);
            if !azp.is_some_and(|a| self.cfg.authorized_parties.iter().any(|p| p == a)) {
                return Err(TokenError::UnauthorizedParty);
            }
        }

        let text = |name: &str| {
            c.get(name)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty() && s.len() <= 255)
                .map(str::to_owned)
        };
        let groups: BTreeSet<String> = c
            .get(&self.cfg.groups_claim)
            .and_then(Value::as_array)
            .map(|l| {
                l.iter()
                    .filter_map(Value::as_str)
                    .map(|g| g.trim_start_matches('/').to_owned())
                    .filter(|g| !g.is_empty() && g.len() <= 255)
                    .take(200)
                    .collect()
            })
            .unwrap_or_default();
        let amr_mfa = c.get("amr").and_then(Value::as_array).is_some_and(|l| {
            l.iter()
                .filter_map(Value::as_str)
                .any(|a| self.cfg.mfa_amr.iter().any(|m| m == a))
        });
        let acr_mfa = c
            .get("acr")
            .and_then(Value::as_str)
            .is_some_and(|a| self.cfg.mfa_acr.iter().any(|m| m == a));

        Ok(Identity {
            issuer: issuer.to_owned(),
            subject: subject.to_owned(),
            username: text(&self.cfg.username_claim),
            email: text("email"),
            name: text("name"),
            groups,
            mfa: amr_mfa || acr_mfa,
            expires_at: exp,
        })
    }
}

/// Is this bearer value shaped like a signed token (three dot-separated parts)? Used to tell
/// OIDC tokens from Apelora API tokens before verifying anything.
pub fn looks_like_jwt(s: &str) -> bool {
    let mut parts = s.split('.');
    matches!((parts.next(), parts.next(), parts.next(), parts.next()), (Some(a), Some(b), Some(c), None) if !a.is_empty() && !b.is_empty() && !c.is_empty())
}

/// The `iss` claim of a signed token, read **without verifying anything**. Only used to pick
/// which configured provider's verifier checks the token; the verifier then checks it fully.
pub fn unverified_issuer(token: &str) -> Option<String> {
    use base64::Engine as _;
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    serde_json::from_slice::<Value>(&bytes)
        .ok()?
        .get("iss")?
        .as_str()
        .map(str::to_owned)
}

/// What a provider publishes about itself (`<issuer>/.well-known/openid-configuration`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct Discovery {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub jwks_uri: String,
}

/// https, or plain http to this computer (a development identity provider). The URL is parsed:
/// plain http needs a host that is exactly `localhost`, `127.0.0.1` or `::1`, without user
/// information, so `http://localhost.example.org` or `http://127.0.0.1@example.org` are refused.
pub fn secure_url(u: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(u) else {
        return false;
    };
    match url.scheme() {
        "https" => url.host_str().is_some_and(|h| !h.is_empty()),
        "http" => {
            url.username().is_empty()
                && url.password().is_none()
                && url
                    .host_str()
                    .is_some_and(|h| matches!(h, "localhost" | "127.0.0.1" | "[::1]"))
        }
        _ => false,
    }
}

/// The client for discovery documents and key sets: a short deadline, and no redirects, so a
/// provider (or anything between) cannot send the request to an address `secure_url` never
/// checked.
fn http_client() -> Result<reqwest::Client, TokenError> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| TokenError::KeysUnavailable(e.to_string()))
}

/// A response with a success status, or the reason (a redirect is not followed and is refused).
fn successful(res: reqwest::Response) -> Result<reqwest::Response, String> {
    let status = res.status();
    if status.is_success() {
        Ok(res)
    } else if status.is_redirection() {
        Err(format!("HTTP {status}: redirects are not followed"))
    } else {
        Err(format!("HTTP {status}"))
    }
}

/// Reads a provider's discovery document and checks that it names the same issuer and secure
/// endpoints; then reads its key set and returns how many signing keys it has.
pub async fn discover(issuer: &str) -> Result<(Discovery, usize), TokenError> {
    let result = discover_once(issuer).await;
    apelora_metrics::IDP_KEY_FETCHES
        .inc(&["discovery", if result.is_ok() { "ok" } else { "error" }]);
    result
}

async fn discover_once(issuer: &str) -> Result<(Discovery, usize), TokenError> {
    let issuer = issuer.trim_end_matches('/');
    if !secure_url(issuer) {
        return Err(TokenError::KeysUnavailable(
            "the issuer must be an https:// URL".into(),
        ));
    }
    let client = http_client()?;
    let unavailable =
        |what: &str, e: &dyn std::fmt::Display| TokenError::KeysUnavailable(format!("{what}: {e}"));
    let res = client
        .get(format!("{issuer}/.well-known/openid-configuration"))
        .send()
        .await
        .map_err(|e| unavailable("discovery document", &e))?;
    let doc: Discovery = successful(res)
        .map_err(|e| unavailable("discovery document", &e))?
        .json()
        .await
        .map_err(|e| unavailable("discovery document", &e))?;
    if doc.issuer.trim_end_matches('/') != issuer {
        return Err(TokenError::KeysUnavailable(format!(
            "the provider calls itself {}, not {issuer}",
            doc.issuer
        )));
    }
    if let Some(bad) = [
        &doc.authorization_endpoint,
        &doc.token_endpoint,
        &doc.jwks_uri,
    ]
    .into_iter()
    .find(|u| !secure_url(u))
    {
        return Err(TokenError::KeysUnavailable(format!(
            "the provider names an insecure endpoint ({bad})"
        )));
    }
    let res = client
        .get(&doc.jwks_uri)
        .send()
        .await
        .map_err(|e| unavailable("key set", &e))?;
    let keys: JwkSet = successful(res)
        .map_err(|e| unavailable("key set", &e))?
        .json()
        .await
        .map_err(|e| unavailable("key set", &e))?;
    Ok((doc, keys.keys.len()))
}

#[cfg(test)]
mod tests;
