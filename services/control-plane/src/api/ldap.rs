// SPDX-License-Identifier: AGPL-3.0-only
//! LDAP over TLS for directory sign-in (ADR 0007): bind as the person, read their entry and
//! groups with that same bind, and say precisely what went wrong when it fails.
//!
//! Rules this module keeps:
//! - Only `ldaps://` or `ldap://` with StartTLS; the certificate and the host name are always
//!   verified, against the directory's own certificate authority when one is given, otherwise
//!   against the system's trusted roots.
//! - What a person types reaches the directory only escaped: RFC 4514 inside a DN, RFC 4515
//!   inside a search filter.
//! - An empty password is refused before anything is sent: an LDAP bind with an empty password
//!   is an anonymous bind and would succeed.
//! - Every exchange has a deadline (10 s for sign-in).
//! - Passwords are never logged, put into errors or kept after the bind.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use apelora_persistence::directories::DirectoryFields;
use ldap3::{LdapConnAsync, LdapConnSettings, LdapError, Scope, SearchEntry};
use rustls::pki_types::CertificateDer;
use rustls::pki_types::pem::PemObject;
use serde::Serialize;
use utoipa::ToSchema;

/// How long a sign-in may take in all: connect, TLS, bind and searches.
pub const SIGN_IN_DEADLINE: Duration = Duration::from_secs(10);
/// How long the settings page waits for a directory to answer the TLS connect check.
pub const REACH_DEADLINE: Duration = Duration::from_secs(5);
/// The placeholder for what the person typed.
pub const USERNAME: &str = "{username}";
/// The placeholder for the person's entry in a group search filter.
pub const DN: &str = "{dn}";
const MAX_GROUPS: usize = 500;

// ---- escaping ---------------------------------------------------------------------------------

/// Escapes a value for use inside a distinguished name (RFC 4514, section 2.4): the special
/// characters `" + , ; < > \` and `=`, a leading `#` or space, a trailing space, and control
/// characters (as `\XX`). The result can never end the attribute value early or add another
/// RDN, so `,ou=admins` stays part of the name.
pub fn escape_dn_value(value: &str) -> String {
    let last = value.chars().count().saturating_sub(1);
    let mut out = String::with_capacity(value.len() + 8);
    for (i, c) in value.chars().enumerate() {
        match c {
            '"' | '+' | ',' | ';' | '<' | '>' | '\\' | '=' => {
                out.push('\\');
                out.push(c);
            }
            '#' if i == 0 => out.push_str("\\#"),
            ' ' if i == 0 || i == last => out.push_str("\\ "),
            c if c.is_control() => {
                let mut buf = [0u8; 4];
                for b in c.encode_utf8(&mut buf).bytes() {
                    out.push_str(&format!("\\{b:02x}"));
                }
            }
            c => out.push(c),
        }
    }
    out
}

/// Escapes a value for use inside a search filter (RFC 4515, section 3): `*`, `(`, `)`, `\`
/// and NUL become `\2a`, `\28`, `\29`, `\5c` and `\00`, so `*)(uid=*` matches only an entry
/// literally named that, never everyone.
pub fn escape_filter_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 8);
    for c in value.chars() {
        match c {
            '*' => out.push_str("\\2a"),
            '(' => out.push_str("\\28"),
            ')' => out.push_str("\\29"),
            '\\' => out.push_str("\\5c"),
            '\0' => out.push_str("\\00"),
            c => out.push(c),
        }
    }
    out
}

/// Reverses RFC 4514 escaping in one attribute value (`\,` and `\2c` both become `,`).
fn unescape_dn_value(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            let hex = bytes
                .get(i + 1..i + 3)
                .and_then(|h| std::str::from_utf8(h).ok())
                .and_then(|h| u8::from_str_radix(h, 16).ok());
            match hex {
                Some(b) => {
                    out.push(b);
                    i += 3;
                }
                None => {
                    out.push(bytes[i + 1]);
                    i += 2;
                }
            }
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).trim().to_owned()
}

/// The attribute type and value of a DN's first RDN (`cn=dev-admins,ou=groups,…` →
/// `("cn", "dev-admins")`). `None` when the text is not a DN.
pub fn first_rdn(dn: &str) -> Option<(String, String)> {
    let mut escaped = false;
    let mut end = dn.len();
    for (i, c) in dn.char_indices() {
        match c {
            _ if escaped => escaped = false,
            '\\' => escaped = true,
            ',' | '+' => {
                end = i;
                break;
            }
            _ => {}
        }
    }
    let (attr, value) = dn[..end].split_once('=')?;
    let attr = attr.trim();
    if attr.is_empty()
        || !attr
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
    {
        return None;
    }
    Some((attr.to_owned(), unescape_dn_value(value)))
}

// ---- settings ---------------------------------------------------------------------------------

/// How the bind name is built from the template.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindForm {
    /// A distinguished name such as `uid={username},ou=people,dc=example,dc=org`.
    Dn,
    /// A principal name such as `{username}@example.org` (or `EXAMPLE\{username}`).
    Principal,
}

pub fn bind_form(template: &str) -> BindForm {
    if template.replace(USERNAME, "").contains('=') {
        BindForm::Dn
    } else {
        BindForm::Principal
    }
}

/// Checks what a person typed as their username. The same rules for every directory.
pub fn check_username(username: &str, form: BindForm) -> Result<(), &'static str> {
    if username.is_empty() || username.chars().count() > 256 {
        return Err("the username must be 1 to 256 characters");
    }
    if username.chars().any(char::is_control) {
        return Err("the username has characters that are not allowed");
    }
    // A principal name is not a DN, so DN escaping does not apply: refuse the characters that
    // would change which realm or domain the name points at.
    if form == BindForm::Principal && username.contains(['@', '\\', '/']) {
        return Err("type the username without a domain");
    }
    Ok(())
}

/// The name Apelora binds with: the template with the escaped username.
pub fn bind_name(template: &str, username: &str) -> String {
    match bind_form(template) {
        BindForm::Dn => template.replace(USERNAME, &escape_dn_value(username)),
        BindForm::Principal => template.replace(USERNAME, username),
    }
}

/// Reads a PEM bundle of certificate authorities (1 to 16 certificates).
pub fn parse_ca(pem: &str) -> Result<Vec<CertificateDer<'static>>, String> {
    let certs = CertificateDer::pem_slice_iter(pem.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("the CA certificate could not be read: {e}"))?;
    match certs.len() {
        0 => Err("the CA certificate field holds no PEM certificate".into()),
        1..=16 => {
            let mut roots = rustls::RootCertStore::empty();
            for c in &certs {
                roots
                    .add(c.clone())
                    .map_err(|e| format!("the CA certificate is not usable: {e}"))?;
            }
            Ok(certs)
        }
        _ => Err("give at most 16 CA certificates".into()),
    }
}

static SYSTEM_ROOTS: LazyLock<Vec<CertificateDer<'static>>> = LazyLock::new(|| {
    let loaded = rustls_native_certs::load_native_certs();
    for e in &loaded.errors {
        tracing::warn!(error = %e, "directory sign-in: a system certificate could not be loaded");
    }
    loaded.certs
});

fn tls_config(ca_pem: Option<&str>) -> Result<Arc<rustls::ClientConfig>, Failure> {
    let mut roots = rustls::RootCertStore::empty();
    match ca_pem {
        Some(pem) => {
            for c in parse_ca(pem).map_err(Failure::Settings)? {
                roots.add(c).map_err(|e| Failure::Settings(e.to_string()))?;
            }
        }
        None => {
            roots.add_parsable_certificates(SYSTEM_ROOTS.iter().cloned());
        }
    }
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| Failure::Settings(e.to_string()))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(config))
}

// ---- failures ---------------------------------------------------------------------------------

/// Why talking to a directory failed, in words an administrator can act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum DirectoryProblem {
    /// Nothing answered at the address (wrong host or port, firewall, server down).
    Unreachable,
    /// The directory did not answer within the deadline.
    Timeout,
    /// The TLS handshake failed for another reason.
    Tls,
    /// The server's certificate is not signed by a trusted authority.
    CertificateUntrusted,
    /// The certificate does not name the host in the URL.
    CertificateNameMismatch,
    /// The server refused StartTLS.
    StartTlsRefused,
    /// The directory refused the username and password.
    BindRefused,
    /// The bind worked but the person's entry could not be found or read.
    EntryNotFound,
    /// The search for the person found more than one entry.
    EntryAmbiguous,
    /// A search failed (for example, the bind may not read groups).
    SearchFailed,
    /// The saved settings cannot be used (for example, the CA certificate).
    Settings,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    Unreachable(String),
    Timeout,
    Tls(String),
    CertificateUntrusted,
    CertificateNameMismatch,
    StartTlsRefused(String),
    BindRefused,
    EntryNotFound,
    EntryAmbiguous(usize),
    SearchFailed(String),
    Settings(String),
}

impl Failure {
    pub fn problem(&self) -> DirectoryProblem {
        match self {
            Failure::Unreachable(_) => DirectoryProblem::Unreachable,
            Failure::Timeout => DirectoryProblem::Timeout,
            Failure::Tls(_) => DirectoryProblem::Tls,
            Failure::CertificateUntrusted => DirectoryProblem::CertificateUntrusted,
            Failure::CertificateNameMismatch => DirectoryProblem::CertificateNameMismatch,
            Failure::StartTlsRefused(_) => DirectoryProblem::StartTlsRefused,
            Failure::BindRefused => DirectoryProblem::BindRefused,
            Failure::EntryNotFound => DirectoryProblem::EntryNotFound,
            Failure::EntryAmbiguous(_) => DirectoryProblem::EntryAmbiguous,
            Failure::SearchFailed(_) => DirectoryProblem::SearchFailed,
            Failure::Settings(_) => DirectoryProblem::Settings,
        }
    }

    /// Whether the directory could not be used at all (as opposed to refusing the person).
    pub fn is_connection(&self) -> bool {
        matches!(
            self,
            Failure::Unreachable(_)
                | Failure::Timeout
                | Failure::Tls(_)
                | Failure::CertificateUntrusted
                | Failure::CertificateNameMismatch
                | Failure::StartTlsRefused(_)
                | Failure::Settings(_)
        )
    }

    /// For administrators (the check result and the server log); never contains a password.
    pub fn message(&self) -> String {
        match self {
            Failure::Unreachable(e) => format!("nothing answered at this address: {e}"),
            Failure::Timeout => "the directory did not answer in time".into(),
            Failure::Tls(e) => format!("the TLS connection failed: {e}"),
            Failure::CertificateUntrusted => "the directory's certificate is not trusted: add the certificate authority that signed it (PEM)".into(),
            Failure::CertificateNameMismatch => "the directory's certificate does not name the host in the URL: use the host name the certificate was issued for".into(),
            Failure::StartTlsRefused(e) => format!("the directory refused StartTLS: {e}"),
            Failure::BindRefused => "the directory refused the username and password".into(),
            Failure::EntryNotFound => "the bind worked, but the person's entry was not found or may not be read with their own bind: check the base DN, the user filter and the directory's access rules".into(),
            Failure::EntryAmbiguous(n) => format!("the user filter found {n} entries; it must find exactly one"),
            Failure::SearchFailed(e) => format!("a search failed: {e}"),
            Failure::Settings(e) => e.clone(),
        }
    }
}

fn tls_failure(e: &rustls::Error) -> Failure {
    use rustls::CertificateError as C;
    match e {
        rustls::Error::InvalidCertificate(C::UnknownIssuer | C::BadSignature) => {
            Failure::CertificateUntrusted
        }
        rustls::Error::InvalidCertificate(
            C::NotValidForName | C::NotValidForNameContext { .. },
        ) => Failure::CertificateNameMismatch,
        other => Failure::Tls(other.to_string()),
    }
}

fn io_failure(e: &std::io::Error) -> Failure {
    if let Some(tls) = e
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<rustls::Error>())
    {
        return tls_failure(tls);
    }
    match e.kind() {
        std::io::ErrorKind::TimedOut => Failure::Timeout,
        std::io::ErrorKind::InvalidData | std::io::ErrorKind::UnexpectedEof => {
            Failure::Tls(format!("the server did not complete a TLS handshake ({e})"))
        }
        _ => Failure::Unreachable(e.to_string()),
    }
}

/// While connecting (TCP, StartTLS, TLS handshake).
fn connect_failure(e: LdapError) -> Failure {
    match e {
        LdapError::Timeout { .. } => Failure::Timeout,
        LdapError::Io { source } => io_failure(&source),
        LdapError::Rustls { source } => tls_failure(&source),
        LdapError::DNSName { source } => Failure::Settings(format!(
            "the host name cannot be checked against a certificate: {source}"
        )),
        LdapError::LdapResult { result } => {
            Failure::StartTlsRefused(format!("result {} {}", result.rc, result.text))
        }
        LdapError::EndOfStream | LdapError::ResultRecv { .. } => {
            Failure::Tls("the server closed the connection during the handshake".into())
        }
        other => Failure::Unreachable(other.to_string()),
    }
}

/// During an operation after the connection is up.
fn op_failure(e: LdapError) -> Failure {
    match e {
        LdapError::Timeout { .. } => Failure::Timeout,
        LdapError::Io { source } => io_failure(&source),
        // invalidCredentials, inappropriateAuthentication
        LdapError::LdapResult { result } if matches!(result.rc, 48 | 49) => Failure::BindRefused,
        // noSuchObject, insufficientAccessRights: the person's own bind may not read it.
        LdapError::LdapResult { result } if matches!(result.rc, 32 | 50) => Failure::EntryNotFound,
        LdapError::LdapResult { result } => {
            Failure::SearchFailed(format!("result {} {}", result.rc, result.text))
        }
        other => Failure::SearchFailed(other.to_string()),
    }
}

// ---- reading an entry ---------------------------------------------------------------------------

/// One entry as the directory returned it; attribute names are matched without regard to case.
#[derive(Debug, Clone, Default)]
pub struct Entry {
    pub dn: String,
    pub text: HashMap<String, Vec<String>>,
    pub binary: HashMap<String, Vec<Vec<u8>>>,
}

impl Entry {
    fn texts(&self, attr: &str) -> Vec<String> {
        self.text
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case(attr))
            .flat_map(|(_, v)| v.iter().cloned())
            .collect()
    }

    fn first_text(&self, attr: &str) -> Option<String> {
        self.texts(attr)
            .into_iter()
            .map(|v| v.trim().to_owned())
            .find(|v| !v.is_empty())
    }

    fn first_binary(&self, attr: &str) -> Option<Vec<u8>> {
        self.binary
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(attr))
            .and_then(|(_, v)| v.first().cloned())
    }

    /// A stable identifier: text as is, binary (such as a GUID) as lowercase hex.
    fn id(&self, attr: &str) -> Option<String> {
        self.first_text(attr).or_else(|| {
            self.first_binary(attr)
                .filter(|b| !b.is_empty())
                .map(|b| b.iter().map(|x| format!("{x:02x}")).collect())
        })
    }
}

fn entry_from(e: ldap3::ResultEntry) -> Option<Entry> {
    // The parser panics on malformed replies; a broken server must not take the request down.
    let parsed =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| SearchEntry::construct(e)));
    parsed.ok().map(|s| Entry {
        dn: s.dn,
        text: s.attrs,
        binary: s.bin_attrs,
    })
}

/// Who the directory says the person is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Person {
    pub dn: String,
    /// The entry's stable id: the id attribute, else `entryUUID`, else `objectGUID` as hex,
    /// else the DN.
    pub subject: String,
    pub username: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
    /// Group names, sorted and without duplicates.
    pub groups: Vec<String>,
}

/// Maps an entry onto a person, with group names from the group attribute (the first RDN's
/// value of each DN, or the value itself) plus any found by the group search. Pure.
pub fn person_from(
    f: &DirectoryFields,
    entry: &Entry,
    typed: &str,
    searched_groups: &[String],
) -> Person {
    let subject = entry
        .id(&f.id_attribute)
        .or_else(|| entry.id("entryUUID"))
        .or_else(|| entry.id("objectGUID"))
        .unwrap_or_else(|| entry.dn.clone());
    let mut groups = BTreeSet::new();
    if let Some(attr) = &f.group_attribute {
        for v in entry.texts(attr) {
            let name = first_rdn(&v).map_or_else(|| v.trim().to_owned(), |(_, value)| value);
            groups.insert(name);
        }
    }
    groups.extend(searched_groups.iter().map(|g| g.trim().to_owned()));
    let groups = groups
        .into_iter()
        .filter(|g| !g.is_empty() && g.chars().count() <= 255)
        .take(MAX_GROUPS)
        .collect();
    Person {
        dn: entry.dn.clone(),
        subject,
        username: entry
            .first_text(&f.username_attribute)
            .unwrap_or_else(|| typed.to_owned()),
        email: entry.first_text(&f.email_attribute),
        display_name: entry.first_text(&f.name_attribute),
        groups,
    }
}

fn person_attributes(f: &DirectoryFields) -> Vec<String> {
    let mut attrs: Vec<String> = vec![
        f.id_attribute.clone(),
        "entryUUID".into(),
        "objectGUID".into(),
        f.username_attribute.clone(),
        f.email_attribute.clone(),
        f.name_attribute.clone(),
    ];
    attrs.extend(f.group_attribute.clone());
    let mut seen = BTreeSet::new();
    attrs.retain(|a| seen.insert(a.to_ascii_lowercase()));
    attrs
}

// ---- talking to the directory -------------------------------------------------------------------

async fn connect(f: &DirectoryFields, deadline: Duration) -> Result<ldap3::Ldap, Failure> {
    let url = url::Url::parse(&f.url).map_err(|e| Failure::Settings(e.to_string()))?;
    let start_tls = match url.scheme() {
        "ldaps" => false,
        "ldap" if f.start_tls => true,
        "ldap" => {
            return Err(Failure::Settings(
                "plain LDAP is refused: use ldaps:// or turn on StartTLS".into(),
            ));
        }
        other => return Err(Failure::Settings(format!("unknown scheme {other}"))),
    };
    let settings = LdapConnSettings::new()
        .set_conn_timeout(deadline)
        .set_config(tls_config(f.ca_pem.as_deref())?)
        .set_starttls(start_tls);
    let (conn, mut ldap) = LdapConnAsync::from_url_with_settings(settings, &url)
        .await
        .map_err(connect_failure)?;
    ldap3::drive!(conn);
    ldap.with_timeout(deadline);
    Ok(ldap)
}

async fn within<T>(
    deadline: Duration,
    work: impl Future<Output = Result<T, Failure>>,
) -> Result<T, Failure> {
    tokio::time::timeout(deadline, work)
        .await
        .unwrap_or(Err(Failure::Timeout))
}

/// Connects (with TLS and its checks) and disconnects: whether the directory can be used.
pub async fn reach(f: &DirectoryFields, deadline: Duration) -> Result<(), Failure> {
    within(deadline, async {
        let mut ldap = connect(f, deadline).await?;
        let _ = ldap.unbind().await;
        Ok(())
    })
    .await
}

/// Binds as the person and reads who they are. The password is used for the bind only.
pub async fn sign_in(
    f: &DirectoryFields,
    username: &str,
    password: &str,
    deadline: Duration,
) -> Result<Person, Failure> {
    let form = bind_form(&f.bind_template);
    if check_username(username, form).is_err() || password.is_empty() {
        return Err(Failure::BindRefused);
    }
    within(deadline, async {
        let mut ldap = connect(f, deadline).await?;
        let result = session(&mut ldap, f, username, password).await;
        let _ = ldap.unbind().await;
        result
    })
    .await
}

async fn session(
    ldap: &mut ldap3::Ldap,
    f: &DirectoryFields,
    username: &str,
    password: &str,
) -> Result<Person, Failure> {
    let name = bind_name(&f.bind_template, username);
    ldap.simple_bind(&name, password)
        .await
        .and_then(ldap3::LdapResult::success)
        .map_err(op_failure)?;

    let attrs = person_attributes(f);
    let (base, scope, filter) = match (&f.user_filter, &f.base_dn) {
        (Some(filter), Some(base)) => (
            base.clone(),
            Scope::Subtree,
            filter.replace(USERNAME, &escape_filter_value(username)),
        ),
        _ => (name.clone(), Scope::Base, "(objectClass=*)".to_owned()),
    };
    let (entries, _) = ldap
        .search(&base, scope, &filter, &attrs)
        .await
        .and_then(ldap3::SearchResult::success)
        .map_err(op_failure)?;
    let mut entries: Vec<Entry> = entries.into_iter().filter_map(entry_from).collect();
    let entry = match entries.len() {
        0 => return Err(Failure::EntryNotFound),
        1 => entries.remove(0),
        n => return Err(Failure::EntryAmbiguous(n)),
    };

    let mut searched = Vec::new();
    if let Some(group_base) = &f.group_base_dn {
        let template = f.group_filter.as_deref().unwrap_or("(member={dn})");
        let filter = template
            .replace(DN, &escape_filter_value(&entry.dn))
            .replace(USERNAME, &escape_filter_value(username));
        let (groups, _) = ldap
            .search(
                group_base,
                Scope::Subtree,
                &filter,
                vec![f.group_name_attribute.as_str()],
            )
            .await
            .and_then(ldap3::SearchResult::success)
            .map_err(|e| match op_failure(e) {
                Failure::EntryNotFound => {
                    Failure::SearchFailed("the person's bind may not read the groups".into())
                }
                other => other,
            })?;
        for g in groups.into_iter().filter_map(entry_from) {
            searched.extend(g.first_text(&f.group_name_attribute));
        }
    }
    Ok(person_from(f, &entry, username, &searched))
}

#[cfg(test)]
mod tests {
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
            first_rdn(&name),
            Some(("uid".into(), "ada,ou=admins".into()))
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
    fn rdns_are_read_and_unescaped() {
        assert_eq!(
            first_rdn("cn=dev-admins,ou=groups,dc=example,dc=test"),
            Some(("cn".into(), "dev-admins".into()))
        );
        assert_eq!(
            first_rdn("CN=Ops\\, Europe,OU=Groups,DC=example,DC=test"),
            Some(("CN".into(), "Ops, Europe".into()))
        );
        assert_eq!(
            first_rdn("cn=a\\2cb,dc=x"),
            Some(("cn".into(), "a,b".into()))
        );
        assert_eq!(first_rdn("plain-group-name"), None);
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
            &["dev-operators".into(), "searched".into()],
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
            vec!["dev-admins", "dev-operators", "plain", "searched"]
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
        assert!(
            parse_ca("-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n").is_err()
        );
    }
}
