// SPDX-License-Identifier: AGPL-3.0-only
//! The support bundle (Console plan, Debugging, P1; EVERY-SIZE.md "Debugging"): one tar file an
//! administrator downloads and hands to whoever helps them, with what is needed to understand a
//! problem and nothing that grants access.
//!
//! Contents: a manifest, versions, the effective settings (values only by the allow-list in
//! `settings`), the health report, preflight, jobs that failed in the last 24 hours with their
//! attempts (never their request bodies, which can hold instance configuration), a metrics
//! snapshot, and a note on logs: the control plane does not keep log lines in memory, so the
//! bundle says where to collect them instead of inventing them. A part the caller may not read,
//! or that fails, is listed in the manifest as omitted with the reason.
//!
//! **Never in a bundle:** tokens, passwords, certificates or keys. Parts are built from allow-listed
//! fields, and every byte also passes through `settings::scrub`, which removes Apelora tokens, PEM
//! blocks, bearer values, JSON web tokens and passwords in URLs. A test plants secrets in every
//! source and checks none reaches the bundle.
//!
//! Needs `support.bundle` (Fabric Administrator). Audited (`support.bundle.downloaded`) before the
//! first byte; a bundle that cannot be audited is not sent. Nothing is uploaded anywhere: it works
//! the same without internet access (ADR 0010).

use apelora_auth::Resource;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Serialize;
use serde_json::{Value, json};

use super::authz::{allow, audit_alone};
use super::deploy::store;
use super::settings::scrub;
use super::{ApiError, AppState, ErrorBody, Principal};

/// Failed jobs from this many hours back.
const FAILED_JOB_HOURS: i64 = 24;
/// At most this many failed jobs.
const FAILED_JOB_LIMIT: i64 = 50;

/// One file of the bundle.
struct Part {
    path: &'static str,
    what: &'static str,
    bytes: Vec<u8>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ManifestFile {
    path: &'static str,
    bytes: usize,
    what: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Omitted {
    what: &'static str,
    reason: String,
}

fn json_part(path: &'static str, what: &'static str, v: &impl Serialize) -> Part {
    let text = serde_json::to_string_pretty(v).unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}"));
    Part {
        path,
        what,
        bytes: scrub(&text).into_bytes(),
    }
}

fn text_part(path: &'static str, what: &'static str, text: &str) -> Part {
    Part {
        path,
        what,
        bytes: scrub(text).into_bytes(),
    }
}

/// Adds a part, or notes why it is missing.
fn take<T: Serialize>(
    parts: &mut Vec<Part>,
    omitted: &mut Vec<Omitted>,
    path: &'static str,
    what: &'static str,
    r: Result<Json<T>, ApiError>,
) {
    match r {
        Ok(Json(v)) => parts.push(json_part(path, what, &v)),
        Err(e) => omitted.push(Omitted {
            what,
            reason: scrub(e.message()),
        }),
    }
}

const LOGS_NOTE: &str = "\
Log lines are not in this bundle.

The Apelora control plane does not keep its log lines in memory; it writes them to standard
error, where the service manager or the container runtime collects them. Collect the last hour
of errors where your installation keeps them, for example:

  journalctl -u apelora-control-plane --since \"-1h\" -p err
  journalctl -u apelora-deployment-worker --since \"-1h\" -p err

Set APELORA_LOG_FORMAT=json for one JSON object per line. Look for the request id
(x-request-id) of a failed request: audit rows and jobs carry it as requestId.
Before you share log lines, check them for secrets: Apelora never logs tokens, passwords,
keys or certificates, but other software on the same server may.
";

const README: &str = "\
Apelora support bundle

Made for whoever helps you with a problem. It holds:
  manifest.json      what is in this bundle, who made it and when, and what was left out
  versions.json      this control plane, its API and the Incus release it is pinned to
  settings.json      the settings it runs with; values only where they are safe to show
  health.json        the health report (control plane, database, workers, queue, Incus)
  preflight.json     the preflight checks
  jobs-failed.json   jobs that failed in the last 24 hours, with each attempt
  metrics.txt        a snapshot of the control plane's metrics
  logs/README.txt    where to find log lines (they are not kept in memory)

It never holds tokens, passwords, certificates or keys, and job request bodies are left out.
Nothing was uploaded anywhere: this file exists only where you saved it.
";

/// Jobs that failed recently with their attempts: allow-listed fields only (no request body).
async fn failed_jobs(state: &AppState) -> Result<Value, ApiError> {
    let pool = store(state)?;
    let jobs =
        apelora_persistence::jobs::failed_jobs(pool, FAILED_JOB_HOURS, FAILED_JOB_LIMIT).await?;
    let mut out = Vec::with_capacity(jobs.len());
    for j in jobs {
        let attempts: Vec<Value> = apelora_persistence::jobs::job_attempts(pool, j.id)
            .await?
            .into_iter()
            .map(|a| {
                json!({
                    "attempt": a.attempt,
                    "worker": a.worker,
                    "startedAt": super::admin::rfc3339(a.started_at),
                    "endedAt": a.ended_at.map(super::admin::rfc3339),
                    "outcome": a.outcome,
                    "error": a.error,
                    "incusOperations": a.incus_operations,
                })
            })
            .collect();
        out.push(json!({
            "id": j.id,
            "kind": j.kind,
            "status": j.status,
            "attempts": j.attempts,
            "maxAttempts": j.max_attempts,
            "lastError": j.last_error,
            "requestId": j.request_id,
            "incusOperations": j.incus_operations,
            "queuedBy": j.queued_by,
            "retryOf": j.retry_of,
            "retriedBy": j.retried_by,
            "createdAt": super::admin::rfc3339(j.created_at),
            "updatedAt": super::admin::rfc3339(j.updated_at),
            "attemptHistory": attempts,
        }));
    }
    Ok(json!({ "hours": FAILED_JOB_HOURS, "limit": FAILED_JOB_LIMIT, "items": out }))
}

/// Builds every part. Parts the caller may not read (they need `cluster.view`) are omitted with
/// the reason rather than failing the bundle.
async fn parts(state: &AppState, p: &Principal) -> (Vec<Part>, Vec<Omitted>) {
    let mut parts = Vec::new();
    let mut omitted = Vec::new();
    let s = || State(state.clone());
    let e = || Extension(p.clone());
    take(
        &mut parts,
        &mut omitted,
        "versions.json",
        "versions of the control plane, its API and the pinned Incus release",
        super::admin::system(s(), e()).await,
    );
    match super::settings::report(state).await {
        Ok(r) => parts.push(json_part(
            "settings.json",
            "effective settings; values only where safe to show",
            &r,
        )),
        Err(err) => omitted.push(Omitted {
            what: "effective settings",
            reason: scrub(err.message()),
        }),
    }
    take(
        &mut parts,
        &mut omitted,
        "health.json",
        "health of the control plane, database, workers, job queue and Incus",
        super::fabric_health::components(s(), e()).await,
    );
    take(
        &mut parts,
        &mut omitted,
        "preflight.json",
        "preflight checks",
        super::preflight(s(), e()).await,
    );
    match failed_jobs(state).await {
        Ok(v) => parts.push(json_part(
            "jobs-failed.json",
            "jobs that failed in the last 24 hours, with their attempts (no request bodies)",
            &v,
        )),
        Err(err) => omitted.push(Omitted {
            what: "failed jobs",
            reason: scrub(err.message()),
        }),
    }
    parts.push(text_part(
        "metrics.txt",
        "metrics snapshot (text exposition format)",
        &super::metrics::snapshot(state).await,
    ));
    parts.push(text_part(
        "logs/README.txt",
        "where to find log lines: the control plane does not keep them in memory",
        LOGS_NOTE,
    ));
    parts.push(text_part("README.txt", "what this bundle holds", README));
    (parts, omitted)
}

// ---- tar (POSIX ustar), written here: a few headers do not justify a new dependency ----------

fn octal(field: &mut [u8], value: u64) {
    let digits = field.len() - 1;
    let s = format!("{value:0digits$o}");
    field[..digits].copy_from_slice(&s.as_bytes()[s.len() - digits..]);
    field[digits] = 0;
}

/// The 512-byte header of a regular file.
pub(super) fn tar_header(path: &str, size: usize, mtime: u64) -> [u8; 512] {
    let mut h = [0u8; 512];
    let name = path.as_bytes();
    h[..name.len().min(100)].copy_from_slice(&name[..name.len().min(100)]);
    octal(&mut h[100..108], 0o644);
    octal(&mut h[108..116], 0);
    octal(&mut h[116..124], 0);
    octal(&mut h[124..136], size as u64);
    octal(&mut h[136..148], mtime);
    h[148..156].copy_from_slice(b"        ");
    h[156] = b'0';
    h[257..263].copy_from_slice(b"ustar\0");
    h[263..265].copy_from_slice(b"00");
    h[265..272].copy_from_slice(b"apelora");
    h[297..304].copy_from_slice(b"apelora");
    let sum: u32 = h.iter().map(|b| u32::from(*b)).sum();
    let s = format!("{sum:06o}\0 ");
    h[148..156].copy_from_slice(s.as_bytes());
    h
}

/// The archive as chunks: a header, the data padded to 512 bytes, per file; two empty blocks.
pub(super) fn tar_chunks(files: Vec<(String, Vec<u8>)>, mtime: u64) -> Vec<Bytes> {
    let mut out = Vec::with_capacity(files.len() * 2 + 1);
    for (path, mut data) in files {
        out.push(Bytes::copy_from_slice(&tar_header(
            &path,
            data.len(),
            mtime,
        )));
        let pad = (512 - data.len() % 512) % 512;
        data.extend(std::iter::repeat_n(0u8, pad));
        out.push(Bytes::from(data));
    }
    out.push(Bytes::from(vec![0u8; 1024]));
    out
}

/// Downloads a support bundle (a tar file, streamed): manifest, versions, effective settings
/// (secrets never included), health, preflight, jobs that failed in the last 24 hours with their
/// attempts, a metrics snapshot and where to find logs. Never tokens, passwords, certificates or
/// keys. Needs `support.bundle`; audited (`support.bundle.downloaded`) before it is sent.
#[utoipa::path(get, path = "/api/v1/support-bundle", tag = "system", operation_id = "support_bundle",
    responses(
        (status = 200, description = "A tar archive", content_type = "application/x-tar", body = String),
        (status = 401, body = ErrorBody), (status = 403, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn bundle(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Response, ApiError> {
    allow(&p, "support.bundle", &Resource::platform())?;
    let pool = store(&state)?;
    let now = time::OffsetDateTime::now_utc();
    let (mut parts, omitted) = parts(&state, &p).await;
    let request = super::requests::current();
    let manifest = json!({
        "format": "apelora-support-bundle/1",
        "createdAt": super::admin::rfc3339(now),
        "createdBy": p.actor_ref(),
        "requestId": request.as_deref(),
        "controlPlaneVersion": env!("CARGO_PKG_VERSION"),
        "apiVersion": apelora_domain::API_VERSION,
        "files": parts.iter().map(|x| ManifestFile { path: x.path, bytes: x.bytes.len(), what: x.what }).collect::<Vec<_>>(),
        "omitted": omitted,
        "logs": "not kept in memory by the control plane; see logs/README.txt",
        "secrets": "none: tokens, passwords, certificates and keys are never written; every file is also scrubbed for anything that looks like one",
    });
    parts.insert(0, json_part("manifest.json", "this list", &manifest));
    // On the record before the first byte: a bundle that cannot be audited is not sent.
    audit_alone(
        pool,
        &p,
        "support.bundle.downloaded",
        "support-bundle",
        "success",
        None,
        &json!({
            "files": parts.iter().map(|x| x.path).collect::<Vec<_>>(),
            "omitted": manifest["omitted"],
        }),
    )
    .await?;
    let stamp = format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    );
    let dir = format!("apelora-support-{stamp}");
    let files = parts
        .into_iter()
        .map(|x| (format!("{dir}/{}", x.path), x.bytes))
        .collect();
    let chunks = tar_chunks(files, now.unix_timestamp().max(0) as u64);
    let body = Body::from_stream(futures_util::stream::iter(
        chunks.into_iter().map(Ok::<_, std::io::Error>),
    ));
    let mut res = (StatusCode::OK, body).into_response();
    let h = res.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/x-tar"),
    );
    if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"{dir}.tar\"")) {
        h.insert(header::CONTENT_DISPOSITION, v);
    }
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(res)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    /// Reads a tar archive back: (path, data) per file, checking every header's checksum.
    pub(crate) fn untar(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
        let mut out = Vec::new();
        let mut at = 0;
        while at + 512 <= bytes.len() {
            let h = &bytes[at..at + 512];
            if h.iter().all(|b| *b == 0) {
                break;
            }
            let field = |r: std::ops::Range<usize>| {
                String::from_utf8_lossy(&h[r])
                    .trim_matches(|c: char| c == '\0' || c == ' ')
                    .to_owned()
            };
            let mut blank = h.to_vec();
            blank[148..156].copy_from_slice(b"        ");
            let sum: u32 = blank.iter().map(|b| u32::from(*b)).sum();
            assert_eq!(
                u32::from_str_radix(&field(148..156), 8).expect("checksum"),
                sum
            );
            assert_eq!(&h[257..262], b"ustar");
            let size = usize::from_str_radix(&field(124..136), 8).expect("size");
            let name = field(0..100);
            at += 512;
            out.push((name, bytes[at..at + size].to_vec()));
            at += size.div_ceil(512) * 512;
        }
        out
    }

    #[test]
    fn the_archive_is_valid_ustar_with_padded_files() {
        let files = vec![
            ("b/one.txt".to_owned(), b"hello".to_vec()),
            ("b/empty".to_owned(), vec![]),
            ("b/block.bin".to_owned(), vec![7u8; 512]),
        ];
        let bytes: Vec<u8> = tar_chunks(files.clone(), 1_800_000_000)
            .into_iter()
            .flat_map(|b| b.to_vec())
            .collect();
        assert_eq!(bytes.len() % 512, 0);
        assert_eq!(untar(&bytes), files);
    }
}
