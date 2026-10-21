// SPDX-License-Identifier: AGPL-3.0-only
//! Image mirrors (ADR 0010): an installation without internet access, or one that must not pull
//! from public servers, maps each upstream image server to a mirror it runs itself, and can refuse
//! every server that is not one of its mirrors. Pins (fingerprints, digests) are kept, so a mirror
//! must serve exactly the pinned image.

use crate::ImageSource;

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

    /// The source with its server replaced by its mirror (alias and pins unchanged). With
    /// `mirrors_only`, a server that is neither mapped nor itself a mirror is refused.
    pub fn apply(&self, source: &ImageSource) -> Result<ImageSource, String> {
        let server = normalise(&source.server);
        let mut out = source.clone();
        if let Some((_, to)) = self.rules.iter().find(|(from, _)| *from == server) {
            out.server = to.clone();
        } else if self.mirrors_only && !self.rules.iter().any(|(_, to)| *to == server) {
            return Err(format!(
                "image server {} is not one of this installation's mirrors (only mirrors may be used)",
                source.server
            ));
        }
        Ok(out)
    }
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

    #[test]
    fn bad_rules_are_refused() {
        assert!(ImageMirrors::parse("https://a.example", false).is_err());
        assert!(ImageMirrors::parse("http://a.example=https://a.mirror", false).is_err());
        assert!(ImageMirrors::parse("https://a.example=https://", false).is_err());
        assert!(ImageMirrors::parse("https://a=https://b,https://a/=https://c", false).is_err());
        assert!(ImageMirrors::parse("", true).is_err());
        assert!(ImageMirrors::parse("", false).expect("valid").is_empty());
    }
}
