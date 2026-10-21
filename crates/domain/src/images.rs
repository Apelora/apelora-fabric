// SPDX-License-Identifier: AGPL-3.0-only
//! Image mirrors (ADR 0010): an installation without internet access, or one that must not pull
//! from public servers, maps each upstream image server to a mirror it runs itself, and can refuse
//! every server that is not one of its mirrors. Pins (fingerprints, digests) are kept, so a mirror
//! must serve exactly the pinned image.

use serde::{Deserialize, Serialize};

use crate::preflight::{Check, CheckStatus, check};
use crate::{ImageProtocol, ImageSource};

/// Upstream image server → mirror, and whether only mirrors may be used.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImageMirrors {
    rules: Vec<(String, String)>,
    mirrors_only: bool,
}

fn normalise(url: &str) -> String {
    url.trim().trim_end_matches('/').to_ascii_lowercase()
}

impl ImageMirrors {
    /// Parses `upstream=mirror` pairs separated by commas, e.g.
    /// `https://images.example.org=https://images.mirror.internal`. Both sides must be `https://`
    /// URLs; an upstream may appear once.
    pub fn parse(rules: &str, mirrors_only: bool) -> Result<Self, String> {
        let mut out = Vec::new();
        for pair in rules.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let (from, to) = pair
                .split_once('=')
                .ok_or_else(|| format!("{pair:?}: expected upstream=mirror"))?;
            let (from, to) = (normalise(from), normalise(to));
            for u in [&from, &to] {
                if !u.starts_with("https://")
                    || u.len() <= "https://".len()
                    || u.contains(char::is_whitespace)
                {
                    return Err(format!("{u:?}: image servers must be https:// URLs"));
                }
                // The pairs are shown to administrators (`GET /api/v1/image-mirrors`), so they
                // must never carry a password.
                let authority = u["https://".len()..].split('/').next().unwrap_or_default();
                if authority.contains('@') {
                    return Err(format!(
                        "{}: credentials do not belong in image server URLs",
                        authority.rsplit('@').next().unwrap_or_default()
                    ));
                }
            }
            if out.iter().any(|(f, _)| f == &from) {
                return Err(format!("{from}: listed twice"));
            }
            out.push((from, to));
        }
        if mirrors_only && out.is_empty() {
            return Err("only mirrors may be used, but no mirror is configured".into());
        }
        Ok(Self {
            rules: out,
            mirrors_only,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty() && !self.mirrors_only
    }

    pub fn len(&self) -> usize {
        self.rules.len()
    }

    pub fn mirrors_only(&self) -> bool {
        self.mirrors_only
    }

    /// The configured `(upstream, mirror)` pairs, normalised (lower case, no trailing `/`).
    pub fn pairs(&self) -> &[(String, String)] {
        &self.rules
    }

    /// What happens to images from `server`: replaced by its mirror, used as it is, or refused.
    pub fn coverage(&self, server: &str) -> Coverage {
        let server = normalise(server);
        if let Some((_, to)) = self.rules.iter().find(|(from, _)| *from == server) {
            Coverage::Mirrored(to.clone())
        } else if self.rules.iter().any(|(_, to)| *to == server) {
            Coverage::Mirrored(server)
        } else if self.mirrors_only {
            Coverage::Refused
        } else {
            Coverage::Upstream
        }
    }

    /// The source with its server replaced by its mirror (alias and pins unchanged). With
    /// `mirrors_only`, a server that is neither mapped nor itself a mirror is refused.
    pub fn apply(&self, source: &ImageSource) -> Result<ImageSource, String> {
        let server = normalise(&source.server);
        let mut out = source.clone();
        match self.coverage(&server) {
            // A source that already names a mirror is kept as written.
            Coverage::Mirrored(to) if to != server => out.server = to,
            Coverage::Mirrored(_) | Coverage::Upstream => {}
            Coverage::Refused => {
                return Err(format!(
                    "image server {} is not one of this installation's mirrors (only mirrors may be used)",
                    source.server
                ));
            }
        }
        Ok(out)
    }
}

/// What this installation does with images from one server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Coverage {
    /// Images come from this mirror (the server itself when it is one of the mirrors).
    Mirrored(String),
    /// No mirror: images come straight from the server.
    Upstream,
    /// No mirror, and only mirrors may be used: deployments that need it are refused.
    Refused,
}

/// The path that shows whether a mirror answers, by protocol: the simplestreams index, or the
/// OCI registry API root (which may answer 401 to an anonymous caller and still be up).
pub fn probe_path(protocol: ImageProtocol) -> &'static str {
    match protocol {
        ImageProtocol::Simplestreams => "/streams/v1/index.json",
        ImageProtocol::Oci => "/v2/",
    }
}

/// Whether a mirror answered its probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum ProbeState {
    Answers,
    Fails,
    NotChecked,
}

/// The result of probing one mirror: its state and a short reason, never internals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct Probe {
    pub state: ProbeState,
    /// e.g. `HTTP 200`, `no answer within 3 s`.
    pub detail: String,
}

/// An image server that deployable module classes use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NeededServer {
    pub server: String,
    pub protocol: ImageProtocol,
    /// Display names of the classes that use it.
    pub classes: Vec<String>,
}

/// A short, stable id fragment for a URL: `https://Images.Example.org/x` → `images.example.org-x`.
fn slug(url: &str) -> String {
    normalise(url)
        .trim_start_matches("https://")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

fn count(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

/// Preflight checks for image mirrors (ADR 0010). Pure: `probe` returns what the caller
/// observed for a mirror URL. Upstream servers are never probed (an installation without
/// internet access cannot reach them, and one with access does not need us to).
pub fn evaluate(
    m: &ImageMirrors,
    needed: &[NeededServer],
    probe: &dyn Fn(&str) -> Probe,
) -> Vec<Check> {
    use CheckStatus::*;
    let servers = count(needed.len(), "image server", "image servers");
    let mirrors = count(m.len(), "mirror", "mirrors");
    let mut out = vec![if m.is_empty() {
        check(
            "image-mirrors",
            "Image mirrors",
            Warn,
            format!(
                "none configured, so the catalog's {servers} are used directly. Fine with internet \
                 access; without it, set APELORA_IMAGE_MIRRORS and APELORA_IMAGE_SERVERS=mirrors-only"
            ),
        )
    } else if m.mirrors_only() {
        check(
            "image-mirrors",
            "Image mirrors",
            Pass,
            format!("{mirrors}; only mirrors may be used"),
        )
    } else {
        check(
            "image-mirrors",
            "Image mirrors",
            Warn,
            format!(
                "{mirrors}, but other image servers are still allowed: set \
                 APELORA_IMAGE_SERVERS=mirrors-only when this installation has no internet access"
            ),
        )
    }];
    if m.is_empty() {
        return out;
    }
    let mirror_check = |id: String, title: String, mirror: &str, context: String| {
        let p = probe(mirror);
        let (status, detail) = match p.state {
            ProbeState::Answers => (Pass, format!("{mirror} answers{context}")),
            ProbeState::Fails => (
                Fail,
                format!("{mirror} does not answer ({}){context}", p.detail),
            ),
            ProbeState::NotChecked => (
                NotChecked,
                format!("{mirror} was not checked ({}){context}", p.detail),
            ),
        };
        check(&id, &title, status, detail)
    };
    let mut used = std::collections::BTreeSet::new();
    for n in needed {
        let id = format!("image-server-{}", slug(&n.server));
        let title = format!("Images from {}", normalise(&n.server));
        let classes = n.classes.join(", ");
        out.push(match m.coverage(&n.server) {
            Coverage::Mirrored(mirror) => {
                let c = mirror_check(id, title, &mirror, format!(" · used by {classes}"));
                used.insert(mirror);
                c
            }
            Coverage::Refused => check(
                &id,
                &title,
                Fail,
                format!("no mirror, and only mirrors may be used: deploying {classes} is refused"),
            ),
            Coverage::Upstream => check(
                &id,
                &title,
                Warn,
                format!("no mirror: {classes} pull images from this server directly"),
            ),
        });
    }
    for (from, to) in m.pairs() {
        if used.contains(to) {
            continue;
        }
        out.push(mirror_check(
            format!("image-mirror-{}", slug(to)),
            format!("Mirror {to}"),
            to,
            format!(" · mirrors {from}, which no catalog class uses"),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ImageProtocol;

    fn src(server: &str) -> ImageSource {
        ImageSource {
            protocol: ImageProtocol::Oci,
            server: server.into(),
            alias: "library/nginx@sha256:aaaa".into(),
            fingerprint: None,
        }
    }

    #[test]
    fn a_mapped_server_is_replaced_and_the_pin_is_kept() {
        let m = ImageMirrors::parse(
            "https://registry.example.org/=https://Registry.Mirror.internal",
            false,
        )
        .expect("valid");
        let out = m
            .apply(&src("https://registry.example.org"))
            .expect("valid");
        assert_eq!(out.server, "https://registry.mirror.internal");
        assert_eq!(out.alias, "library/nginx@sha256:aaaa");
    }

    #[test]
    fn unmapped_servers_pass_unless_only_mirrors_are_allowed() {
        let open = ImageMirrors::parse("https://a.example=https://a.mirror", false).expect("valid");
        assert_eq!(
            open.apply(&src("https://b.example")).expect("valid").server,
            "https://b.example"
        );
        let closed =
            ImageMirrors::parse("https://a.example=https://a.mirror", true).expect("valid");
        assert!(
            closed
                .apply(&src("https://b.example"))
                .expect_err("refused")
                .contains("not one of this installation's mirrors")
        );
        // A source that already names a mirror is fine.
        assert_eq!(
            closed
                .apply(&src("https://a.mirror/"))
                .expect("valid")
                .server,
            "https://a.mirror/"
        );
    }

    fn needed(server: &str, protocol: ImageProtocol, classes: &[&str]) -> NeededServer {
        NeededServer {
            server: server.into(),
            protocol,
            classes: classes.iter().map(|c| (*c).to_owned()).collect(),
        }
    }

    fn status_of(checks: &[Check], id: &str) -> (CheckStatus, String) {
        checks
            .iter()
            .find(|c| c.id == id)
            .map(|c| (c.status, c.detail.clone()))
            .expect(id)
    }

    fn answers(_: &str) -> Probe {
        Probe {
            state: ProbeState::Answers,
            detail: "HTTP 200".into(),
        }
    }

    #[test]
    fn coverage_names_the_mirror_or_says_upstream_or_refused() {
        let open = ImageMirrors::parse("https://a.example=https://a.mirror", false).expect("ok");
        assert_eq!(
            open.coverage("https://A.example/"),
            Coverage::Mirrored("https://a.mirror".into())
        );
        assert_eq!(
            open.coverage("https://a.mirror"),
            Coverage::Mirrored("https://a.mirror".into())
        );
        assert_eq!(open.coverage("https://b.example"), Coverage::Upstream);
        let closed = ImageMirrors::parse("https://a.example=https://a.mirror", true).expect("ok");
        assert_eq!(closed.coverage("https://b.example"), Coverage::Refused);
        assert_eq!(probe_path(ImageProtocol::Oci), "/v2/");
        assert_eq!(
            probe_path(ImageProtocol::Simplestreams),
            "/streams/v1/index.json"
        );
    }

    #[test]
    fn nothing_configured_is_one_warning_and_nothing_is_probed() {
        let probed = std::cell::Cell::new(0);
        let checks = evaluate(
            &ImageMirrors::default(),
            &[needed(
                "https://images.example",
                ImageProtocol::Simplestreams,
                &["Debian"],
            )],
            &|u| {
                probed.set(probed.get() + 1);
                answers(u)
            },
        );
        assert_eq!(checks.len(), 1);
        let (status, detail) = status_of(&checks, "image-mirrors");
        assert_eq!(status, CheckStatus::Warn);
        assert!(detail.contains("1 image server are used"), "{detail}");
        assert_eq!(probed.get(), 0);
    }

    #[test]
    fn mirrors_are_checked_per_server_and_unused_mirrors_are_still_probed() {
        let m = ImageMirrors::parse(
            "https://images.example=https://images.mirror,https://registry.example=https://registry.mirror,https://old.example=https://old.mirror",
            true,
        )
        .expect("ok");
        let needed = [
            needed(
                "https://images.example",
                ImageProtocol::Simplestreams,
                &["Debian", "Linux"],
            ),
            needed("https://registry.example", ImageProtocol::Oci, &["NGINX"]),
            needed("https://other.example", ImageProtocol::Oci, &["Cache"]),
        ];
        let checks = evaluate(&m, &needed, &|url| {
            if url == "https://registry.mirror" {
                Probe {
                    state: ProbeState::Fails,
                    detail: "no answer within 3 s".into(),
                }
            } else {
                answers(url)
            }
        });
        assert_eq!(status_of(&checks, "image-mirrors").0, CheckStatus::Pass);
        let (s, d) = status_of(&checks, "image-server-images.example");
        assert_eq!(s, CheckStatus::Pass);
        assert!(
            d.contains("https://images.mirror answers") && d.contains("Debian, Linux"),
            "{d}"
        );
        let (s, d) = status_of(&checks, "image-server-registry.example");
        assert_eq!(s, CheckStatus::Fail);
        assert!(d.contains("no answer within 3 s"), "{d}");
        let (s, d) = status_of(&checks, "image-server-other.example");
        assert_eq!(s, CheckStatus::Fail);
        assert!(d.contains("deploying Cache is refused"), "{d}");
        let (s, d) = status_of(&checks, "image-mirror-old.mirror");
        assert_eq!(s, CheckStatus::Pass);
        assert!(d.contains("which no catalog class uses"), "{d}");
        assert_eq!(crate::preflight::overall(&checks), CheckStatus::Fail);

        // Not mirrors-only: an unmapped server is a warning, and so is the mode.
        let open =
            ImageMirrors::parse("https://images.example=https://images.mirror", false).expect("ok");
        let checks = evaluate(&open, &needed, &answers);
        assert_eq!(status_of(&checks, "image-mirrors").0, CheckStatus::Warn);
        assert_eq!(
            status_of(&checks, "image-server-other.example").0,
            CheckStatus::Warn
        );
    }

    #[test]
    fn bad_rules_are_refused() {
        assert!(ImageMirrors::parse("https://a.example", false).is_err());
        assert!(ImageMirrors::parse("http://a.example=https://a.mirror", false).is_err());
        assert!(ImageMirrors::parse("https://a.example=https://", false).is_err());
        assert!(ImageMirrors::parse("https://a=https://b,https://a/=https://c", false).is_err());
        assert!(ImageMirrors::parse("", true).is_err());
        let creds = ImageMirrors::parse("https://a.example=https://user:secret@a.mirror", false)
            .expect_err("credentials refused");
        assert!(
            creds.contains("credentials") && !creds.contains("secret"),
            "{creds}"
        );
        assert!(ImageMirrors::parse("", false).expect("valid").is_empty());
    }
}
