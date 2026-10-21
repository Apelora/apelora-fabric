// SPDX-License-Identifier: AGPL-3.0-only
//! Image mirrors (ADR 0010, docs/architecture/DISCONNECTED.md): which image servers the catalog's
//! deployable classes use, whether this installation mirrors each one, and whether the mirrors
//! answer. Configuration stays in the environment (`APELORA_IMAGE_MIRRORS`,
//! `APELORA_IMAGE_SERVERS`); this is read-only. Only mirrors are probed, never upstream servers,
//! and probe results are kept for a minute so the check cannot be used to flood a mirror.

use std::collections::{BTreeMap, HashMap};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use apelora_domain::images::{self, Coverage, ImageMirrors, NeededServer, Probe, ProbeState};
use apelora_domain::preflight::{Check, CheckStatus};
use apelora_domain::{ImageProtocol, Permission};
use axum::extract::State;
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::{ApiError, AppState, ErrorBody, Principal, require};

/// How long a probe result is reused.
pub const PROBE_TTL: Duration = Duration::from_secs(60);
/// How long a mirror has to answer.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// The images of the catalog's deployable classes, generated from the module catalog by
/// `npm run catalog:doc` (a catalog test fails when the file is stale).
const CATALOG_IMAGES: &str =
    include_str!("../../../../packages/module-catalog/catalog-images.json");

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CatalogImage {
    class: String,
    display_name: String,
    #[serde(default)]
    setting: Option<String>,
    protocol: ImageProtocol,
    server: String,
    alias: String,
}

#[derive(Deserialize)]
struct CatalogFile {
    images: Vec<CatalogImage>,
}

static CATALOG: LazyLock<Vec<CatalogImage>> = LazyLock::new(|| {
    serde_json::from_str::<CatalogFile>(CATALOG_IMAGES)
        .map(|f| f.images)
        .unwrap_or_else(|e| {
            tracing::error!(error = %e, "catalog-images.json cannot be read; the image-mirror check sees no classes");
            Vec::new()
        })
});

/// Whether every image server may be used, or only the mirrors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ImageServerMode {
    Any,
    MirrorsOnly,
}

/// What happens to images from a server here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum CoverageKind {
    /// A mirror serves them.
    Mirrored,
    /// No mirror: they come from the server itself.
    Upstream,
    /// No mirror, and only mirrors may be used: deployments that need them are refused.
    Refused,
}

/// One configured mirror and whether it answered.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MirrorStatus {
    /// The upstream image server it stands in for.
    pub upstream: String,
    pub mirror: String,
    pub probe: Probe,
    /// RFC 3339: when the probe ran (results are reused for a minute).
    pub checked_at: Option<String>,
}

/// An image a class uses, by setting.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClassImage {
    /// Class id, e.g. `nginx`.
    pub class: String,
    pub display_name: String,
    /// The image-selecting setting, e.g. `channel = stable`.
    pub setting: Option<String>,
    /// Alias, or repository with its pinned digest.
    pub alias: String,
}

/// An image server the catalog's deployable classes use, and how this installation covers it.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImageServerUse {
    pub server: String,
    pub protocol: ImageProtocol,
    pub coverage: CoverageKind,
    /// The mirror that serves it, when mirrored.
    pub mirror: Option<String>,
    pub images: Vec<ClassImage>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImageMirrorsReport {
    /// `mirrors-only` when `APELORA_IMAGE_SERVERS=mirrors-only`.
    pub mode: ImageServerMode,
    /// `APELORA_IMAGE_MIRRORS` as this control plane read it at start. The worker reads its own
    /// environment, which must say the same.
    pub mirrors: Vec<MirrorStatus>,
    pub servers: Vec<ImageServerUse>,
    /// The same checks Preflight shows.
    pub checks: Vec<Check>,
    /// The worst status among the checks.
    pub overall: CheckStatus,
}

#[derive(Debug, Clone)]
struct Probed {
    probe: Probe,
    checked_at: String,
    at: Instant,
}

/// Probes mirrors over HTTPS with a short timeout and keeps each result for [`PROBE_TTL`].
/// Callers that arrive while a probe runs wait for it rather than starting another.
pub struct Prober {
    client: Option<reqwest::Client>,
    ttl: Duration,
    timeout: Duration,
    cache: tokio::sync::Mutex<HashMap<String, Probed>>,
}

impl Default for Prober {
    fn default() -> Self {
        Self::new(PROBE_TTL, PROBE_TIMEOUT)
    }
}

impl Prober {
    pub fn new(ttl: Duration, timeout: Duration) -> Self {
        // Redirects are not followed: a mirror answers itself. Certificates are verified against
        // the system's trust store, so an internal certificate authority works once installed.
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .connect_timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .inspect_err(|e| tracing::error!(error = %e, "cannot build the mirror probe client"))
            .ok();
        Self {
            client,
            ttl,
            timeout,
            cache: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    /// The probe result for each mirror, probing those without a recent result. `protocols`
    /// empty means unknown: the mirror answers if either protocol's path answers.
    async fn probe_all(
        &self,
        targets: &BTreeMap<String, Vec<ImageProtocol>>,
    ) -> HashMap<String, Probed> {
        let mut cache = self.cache.lock().await;
        let mut set = tokio::task::JoinSet::new();
        for (mirror, protocols) in targets {
            if cache.get(mirror).is_some_and(|p| p.at.elapsed() < self.ttl) {
                continue;
            }
            let (client, mirror, protocols, timeout) = (
                self.client.clone(),
                mirror.clone(),
                protocols.clone(),
                self.timeout,
            );
            set.spawn(async move {
                let probe = match client {
                    Some(c) => probe_mirror(&c, &mirror, &protocols, timeout).await,
                    None => Probe {
                        state: ProbeState::NotChecked,
                        detail: "the probe client could not be started".into(),
                    },
                };
                (mirror, probe)
            });
        }
        while let Some(done) = set.join_next().await {
            let Ok((mirror, probe)) = done else { continue };
            if probe.state == ProbeState::Fails {
                tracing::warn!(%mirror, detail = %probe.detail, "image mirror does not answer");
            }
            cache.insert(
                mirror,
                Probed {
                    probe,
                    checked_at: jiff::Timestamp::now().to_string(),
                    at: Instant::now(),
                },
            );
        }
        cache.retain(|m, _| targets.contains_key(m));
        cache.clone()
    }
}

async fn probe_mirror(
    client: &reqwest::Client,
    mirror: &str,
    protocols: &[ImageProtocol],
    timeout: Duration,
) -> Probe {
    let (paths, all): (Vec<ImageProtocol>, bool) = if protocols.is_empty() {
        (
            vec![ImageProtocol::Simplestreams, ImageProtocol::Oci],
            false,
        )
    } else {
        (protocols.to_vec(), true)
    };
    let mut results = Vec::new();
    for protocol in paths {
        results.push(probe_url(client, mirror, protocol, timeout).await);
    }
    let answers = |p: &Probe| p.state == ProbeState::Answers;
    let ok = if all {
        results.iter().all(answers)
    } else {
        results.iter().any(answers)
    };
    let pick = |want: bool| results.iter().find(|p| answers(p) == want).cloned();
    pick(ok).unwrap_or_else(|| Probe {
        state: ProbeState::NotChecked,
        detail: "nothing to check".into(),
    })
}

/// One GET of a mirror's probe path. 200 answers; an OCI registry may also answer 401 (it wants
/// a token) and still be up. Errors become short reasons; the full error goes to the log only.
pub(crate) async fn probe_url(
    client: &reqwest::Client,
    mirror: &str,
    protocol: ImageProtocol,
    timeout: Duration,
) -> Probe {
    let url = format!(
        "{}{}",
        mirror.trim_end_matches('/'),
        images::probe_path(protocol)
    );
    match client.get(&url).send().await {
        Ok(r) => {
            let code = r.status().as_u16();
            let up = code == 200 || (protocol == ImageProtocol::Oci && code == 401);
            Probe {
                state: if up {
                    ProbeState::Answers
                } else {
                    ProbeState::Fails
                },
                detail: format!("HTTP {code} from {}", images::probe_path(protocol)),
            }
        }
        Err(e) => {
            tracing::debug!(%url, error = %e, "image mirror probe failed");
            let detail = if e.is_timeout() {
                format!("no answer within {} s", timeout.as_secs())
            } else if e.is_connect() {
                "cannot connect: check the address, port and TLS certificate".to_owned()
            } else {
                "the request failed".to_owned()
            };
            Probe {
                state: ProbeState::Fails,
                detail,
            }
        }
    }
}

/// Image servers the catalog's deployable classes use, grouped by server and protocol.
fn needed() -> Vec<(String, ImageProtocol, Vec<&'static CatalogImage>)> {
    let mut by: BTreeMap<(String, &'static str), (ImageProtocol, Vec<&'static CatalogImage>)> =
        BTreeMap::new();
    for i in CATALOG.iter() {
        let key = match i.protocol {
            ImageProtocol::Simplestreams => "simplestreams",
            ImageProtocol::Oci => "oci",
        };
        by.entry((i.server.trim_end_matches('/').to_ascii_lowercase(), key))
            .or_insert_with(|| (i.protocol, Vec::new()))
            .1
            .push(i);
    }
    by.into_iter()
        .map(|((server, _), (protocol, images))| (server, protocol, images))
        .collect()
}

/// The full report: configuration, coverage per server, probes and checks.
pub(super) async fn report(state: &AppState) -> ImageMirrorsReport {
    report_for(&state.image_mirrors, &state.prober).await
}

pub(crate) async fn report_for(m: &ImageMirrors, prober: &Prober) -> ImageMirrorsReport {
    let needed = needed();
    // Which protocol each mirror serves, from the classes whose servers it stands in for.
    let mut targets: BTreeMap<String, Vec<ImageProtocol>> = m
        .pairs()
        .iter()
        .map(|(_, to)| (to.clone(), Vec::new()))
        .collect();
    let servers: Vec<ImageServerUse> = needed
        .iter()
        .map(|(server, protocol, imgs)| {
            let (coverage, mirror) = match m.coverage(server) {
                Coverage::Mirrored(to) => {
                    let p = targets.entry(to.clone()).or_default();
                    if !p.contains(protocol) {
                        p.push(*protocol);
                    }
                    (CoverageKind::Mirrored, Some(to))
                }
                Coverage::Upstream => (CoverageKind::Upstream, None),
                Coverage::Refused => (CoverageKind::Refused, None),
            };
            ImageServerUse {
                server: server.clone(),
                protocol: *protocol,
                coverage,
                mirror,
                images: imgs
                    .iter()
                    .map(|i| ClassImage {
                        class: i.class.clone(),
                        display_name: i.display_name.clone(),
                        setting: i.setting.clone(),
                        alias: i.alias.clone(),
                    })
                    .collect(),
            }
        })
        .collect();
    let probed = if targets.is_empty() {
        HashMap::new()
    } else {
        prober.probe_all(&targets).await
    };
    let probe_of = |mirror: &str| {
        probed
            .get(mirror)
            .map(|p| p.probe.clone())
            .unwrap_or(Probe {
                state: ProbeState::NotChecked,
                detail: "not probed".into(),
            })
    };
    let domain_needed: Vec<NeededServer> = needed
        .iter()
        .map(|(server, protocol, imgs)| {
            let mut classes: Vec<String> = Vec::new();
            for i in imgs {
                if !classes.contains(&i.display_name) {
                    classes.push(i.display_name.clone());
                }
            }
            NeededServer {
                server: server.clone(),
                protocol: *protocol,
                classes,
            }
        })
        .collect();
    let checks = images::evaluate(m, &domain_needed, &probe_of);
    ImageMirrorsReport {
        mode: if m.mirrors_only() {
            ImageServerMode::MirrorsOnly
        } else {
            ImageServerMode::Any
        },
        mirrors: m
            .pairs()
            .iter()
            .map(|(from, to)| MirrorStatus {
                upstream: from.clone(),
                mirror: to.clone(),
                probe: probe_of(to),
                checked_at: probed.get(to).map(|p| p.checked_at.clone()),
            })
            .collect(),
        servers,
        overall: apelora_domain::preflight::overall(&checks),
        checks,
    }
}

/// Image mirrors: the configuration this control plane runs with (upstream → mirror pairs and
/// whether only mirrors may be used; URLs, not secrets), whether each mirror answers (probed at
/// most once a minute), and the image servers each deployable catalog class needs with whether
/// it is covered. Read-only: configuration stays in the environment. Needs `read-inventory`.
#[utoipa::path(get, path = "/api/v1/image-mirrors", tag = "system", operation_id = "image_mirrors",
    responses((status = 200, body = ImageMirrorsReport), (status = 401, body = ErrorBody), (status = 403, body = ErrorBody)))]
pub(super) async fn image_mirrors(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Json<ImageMirrorsReport>, ApiError> {
    require(&p, Permission::ReadInventory)?;
    Ok(Json(report(&state).await))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn the_generated_catalog_list_is_read() {
        assert!(!CATALOG.is_empty(), "catalog-images.json has no images");
        let n = needed();
        assert!(
            n.iter()
                .any(|(s, p, _)| s == "https://docker.io" && *p == ImageProtocol::Oci),
            "{n:?}"
        );
        let lxc = n
            .iter()
            .find(|(s, _, _)| s == "https://images.linuxcontainers.org")
            .expect("a simplestreams server");
        assert_eq!(lxc.1, ImageProtocol::Simplestreams);
        assert!(lxc.2.iter().any(|i| i.class == "debian"));
    }

    /// A local server that counts requests and answers `status` on the probe paths.
    async fn mirror(status: u16) -> (String, Arc<AtomicUsize>) {
        let hits = Arc::new(AtomicUsize::new(0));
        let h = hits.clone();
        let app = axum::Router::new().fallback(move || {
            let h = h.clone();
            async move {
                h.fetch_add(1, Ordering::SeqCst);
                axum::http::StatusCode::from_u16(status).expect("status")
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move { axum::serve(listener, app).await });
        (format!("http://{addr}"), hits)
    }

    #[tokio::test]
    async fn probes_say_answers_or_why_not_and_are_cached() {
        let prober = Prober::new(Duration::from_secs(60), Duration::from_secs(2));
        let (ok, ok_hits) = mirror(200).await;
        let (oci, _) = mirror(401).await;
        let (broken, _) = mirror(500).await;
        let targets: BTreeMap<String, Vec<ImageProtocol>> = [
            (ok.clone(), vec![ImageProtocol::Simplestreams]),
            (oci.clone(), vec![ImageProtocol::Oci]),
            (broken.clone(), vec![ImageProtocol::Simplestreams]),
            // Nothing listens on port 1.
            ("http://127.0.0.1:1".to_owned(), vec![]),
        ]
        .into();
        let r = prober.probe_all(&targets).await;
        assert_eq!(r[&ok].probe.state, ProbeState::Answers);
        assert_eq!(
            r[&oci].probe.state,
            ProbeState::Answers,
            "401 means the registry is up"
        );
        assert_eq!(r[&broken].probe.state, ProbeState::Fails);
        assert!(r[&broken].probe.detail.contains("HTTP 500"));
        let refused = &r["http://127.0.0.1:1"].probe;
        assert_eq!(refused.state, ProbeState::Fails);
        assert!(refused.detail.starts_with("cannot connect"), "{refused:?}");
        assert!(
            !refused.detail.contains("127.0.0.1"),
            "no internals: {refused:?}"
        );
        // A second check within the minute reuses the result.
        let again = prober.probe_all(&targets).await;
        assert_eq!(again[&ok].checked_at, r[&ok].checked_at);
        assert_eq!(ok_hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn the_report_shows_coverage_per_server_and_never_probes_upstream_servers() {
        let prober = Prober::new(Duration::from_secs(60), Duration::from_secs(1));
        let none = report_for(&ImageMirrors::default(), &prober).await;
        assert_eq!(none.mode, ImageServerMode::Any);
        assert!(none.mirrors.is_empty());
        assert!(
            none.servers
                .iter()
                .all(|s| s.coverage == CoverageKind::Upstream)
        );
        assert_eq!(none.checks.len(), 1);
        assert_eq!(none.overall, CheckStatus::Warn);
        assert!(prober.cache.lock().await.is_empty(), "nothing was probed");

        // Mirrors-only with one mirror that does not answer (nothing listens on port 1).
        let m = ImageMirrors::parse(
            "https://images.linuxcontainers.org=https://127.0.0.1:1",
            true,
        )
        .expect("valid");
        let r = report_for(&m, &prober).await;
        assert_eq!(r.mode, ImageServerMode::MirrorsOnly);
        assert_eq!(r.mirrors.len(), 1);
        assert_eq!(r.mirrors[0].probe.state, ProbeState::Fails);
        assert!(r.mirrors[0].checked_at.is_some());
        let lxc = r
            .servers
            .iter()
            .find(|s| s.server == "https://images.linuxcontainers.org")
            .expect("lxc");
        assert_eq!(
            (lxc.coverage, lxc.mirror.as_deref()),
            (CoverageKind::Mirrored, Some("https://127.0.0.1:1"))
        );
        let docker = r
            .servers
            .iter()
            .find(|s| s.server == "https://docker.io")
            .expect("docker");
        assert_eq!(docker.coverage, CoverageKind::Refused);
        assert_eq!(r.overall, CheckStatus::Fail);
        let cache = prober.cache.lock().await;
        assert_eq!(
            cache.keys().collect::<Vec<_>>(),
            vec!["https://127.0.0.1:1"],
            "only the mirror was probed"
        );
    }
}
