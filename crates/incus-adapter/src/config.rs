// SPDX-License-Identifier: AGPL-3.0-only
//! Chooses the Incus provider from environment variables (shared by the control plane and
//! the deployment worker).

use crate::{IncusClient, IncusConfig, MockIncus, Provider};

pub const URL: &str = "APELORA_INCUS_URL";
pub const CLIENT_CERT: &str = "APELORA_INCUS_CLIENT_CERT";
pub const CLIENT_KEY: &str = "APELORA_INCUS_CLIENT_KEY";
pub const SERVER_CERT: &str = "APELORA_INCUS_SERVER_CERT";
/// Optional: name to verify the server certificate against (default: URL host, else "unspecified").
pub const TLS_SERVER_NAME: &str = "APELORA_INCUS_TLS_SERVER_NAME";
/// Optional, control plane only: an unrestricted identity for cluster administration.
pub const ADMIN_CLIENT_CERT: &str = "APELORA_INCUS_ADMIN_CLIENT_CERT";
pub const ADMIN_CLIENT_KEY: &str = "APELORA_INCUS_ADMIN_CLIENT_KEY";

/// No `APELORA_INCUS_URL`: labelled mock data. URL set: all three certificate paths are
/// required and must be readable, otherwise startup fails with a clear message.
pub fn provider_from_env(get: impl Fn(&str) -> Option<String>) -> Result<Provider, String> {
    let Some(url) = get(URL).filter(|u| !u.trim().is_empty()) else {
        return Ok(Provider::Mock(MockIncus::default()));
    };
    connect(&get, url, CLIENT_CERT, CLIENT_KEY)
}

/// The identity for cluster administration (join tokens), which Incus allows only to an
/// unrestricted client certificate. It is kept apart from the project-restricted identity used
/// for deployments and loaded only by the control plane, never by the deployment worker.
/// `Ok(None)`: not configured, so cluster administration is unavailable. With the mock
/// provider the mock itself is used.
pub fn admin_provider_from_env(
    get: impl Fn(&str) -> Option<String>,
    deploy: &Provider,
) -> Result<Option<Provider>, String> {
    if matches!(deploy, Provider::Mock(_)) {
        return Ok(Some(deploy.clone()));
    }
    let set = |k: &str| get(k).is_some_and(|v| !v.trim().is_empty());
    match (set(ADMIN_CLIENT_CERT), set(ADMIN_CLIENT_KEY)) {
        (false, false) => Ok(None),
        (true, true) => {
            let url = get(URL).unwrap_or_default();
            connect(&get, url, ADMIN_CLIENT_CERT, ADMIN_CLIENT_KEY).map(Some)
        }
        _ => Err(format!(
            "{ADMIN_CLIENT_CERT} and {ADMIN_CLIENT_KEY} must be set together"
        )),
    }
}

fn connect(
    get: &impl Fn(&str) -> Option<String>,
    url: String,
    cert: &str,
    key: &str,
) -> Result<Provider, String> {
    let read = |k: &str| -> Result<Vec<u8>, String> {
        let path = get(k)
            .filter(|p| !p.trim().is_empty())
            .ok_or_else(|| format!("{URL} is set, so {k} is required"))?;
        std::fs::read(&path).map_err(|e| format!("{k}: cannot read {path}: {e}"))
    };
    let cfg = IncusConfig {
        url,
        client_cert_pem: read(cert)?,
        client_key_pem: read(key)?,
        server_cert_pem: read(SERVER_CERT)?,
        tls_server_name: get(TLS_SERVER_NAME).filter(|n| !n.trim().is_empty()),
    };
    IncusClient::mtls(&cfg)
        .map(Provider::Incus)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use apelora_domain::ProviderMode;

    #[test]
    fn no_url_means_mock() {
        assert_eq!(
            provider_from_env(|_| None).expect("provider").mode(),
            ProviderMode::Mock
        );
    }

    #[test]
    fn admin_identity_is_optional_but_complete() {
        let mock = Provider::Mock(MockIncus::default());
        assert!(matches!(
            admin_provider_from_env(|_| None, &mock),
            Ok(Some(Provider::Mock(_)))
        ));
        let real = Provider::Incus(crate::IncusClient::plain_for_tests("http://127.0.0.1:1"));
        assert!(matches!(admin_provider_from_env(|_| None, &real), Ok(None)));
        let half = admin_provider_from_env(
            |k| (k == ADMIN_CLIENT_CERT).then(|| "/x/admin.crt".to_owned()),
            &real,
        )
        .expect_err("cert without key");
        assert!(half.contains("must be set together"), "{half}");
    }

    #[test]
    fn url_without_certificates_refuses_to_start() {
        let err =
            provider_from_env(|k| (k == URL).then(|| "https://incus.invalid:8443".to_owned()))
                .expect_err("must fail");
        assert!(err.contains(CLIENT_CERT), "{err}");
    }

    #[test]
    fn unreadable_certificate_paths_are_reported() {
        let err = provider_from_env(|k| {
            Some(if k == URL {
                "https://incus.invalid:8443".to_owned()
            } else {
                "/nonexistent/apelora.pem".to_owned()
            })
        })
        .expect_err("must fail");
        assert!(
            err.contains("cannot read /nonexistent/apelora.pem"),
            "{err}"
        );
    }
}
