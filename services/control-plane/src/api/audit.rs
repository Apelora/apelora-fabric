// SPDX-License-Identifier: AGPL-3.0-only
//! The audit trail's read side (ADR 0011, EVERY-SIZE.md "Auditing"): the filtered, paged list
//! and the export. Both need `audit.read`; readers who hold it below platform level see only
//! the events of their organisations, projects and applications. Every filter is served by an
//! index (migration 0022). An export is itself audited (`audit.exported`) before its first byte
//! is sent.

use apelora_auth::AccessFilter;
use apelora_persistence::identity::{AuditQuery, AuditRow, audit_history};
use axum::Json;
use axum::body::{Body, Bytes};
use axum::extract::{Extension, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};
use time::OffsetDateTime;
use utoipa::IntoParams;

use super::authz::{audit_alone, filter_for};
use super::{ApiError, AppState, AuditItem, AuditList, Auth, ErrorBody, Principal};

/// Events per database read while exporting.
const EXPORT_PAGE: i64 = 500;
/// Longest text a filter may carry.
const MAX_FILTER: usize = 200;

/// Filters of the audit list and export. Every one narrows; leave one out to not filter by it.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct AuditFilter {
    /// Who acted: their stable id (`user:<uuid>`, `service:<uuid>`, `token:<name>`) or, ignoring
    /// case, the name they had when they acted.
    pub actor: Option<String>,
    /// What was acted on, exactly as recorded (e.g. `local-account:ada` or a group's name).
    pub target: Option<String>,
    /// Only actions starting with this, e.g. `role.` or `emergency_access.`.
    pub action: Option<String>,
    /// `success`, `denied` or `failed`.
    pub outcome: Option<String>,
    /// RFC 3339: events at or after this time.
    pub from: Option<String>,
    /// RFC 3339: events before this time.
    pub to: Option<String>,
    /// Only events older than this id (paging: the id of the last event of the previous page).
    pub before: Option<i64>,
    /// Events per page, 1 to 500 (default 100). Not used by the export.
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ExportFormat {
    /// `csv` or `jsonl` (one JSON object per line, as the list returns them). Default `jsonl`.
    pub format: Option<String>,
}

fn bad(message: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, "invalid_request", message)
}

fn text(v: Option<String>, what: &str) -> Result<Option<String>, ApiError> {
    let Some(v) = v.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    if v.chars().count() > MAX_FILTER || v.chars().any(char::is_control) {
        return Err(bad(format!(
            "{what} is at most {MAX_FILTER} characters, without control characters"
        )));
    }
    Ok(Some(v))
}

fn time(v: Option<String>, what: &str) -> Result<Option<OffsetDateTime>, ApiError> {
    v.map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .map(|s| {
            OffsetDateTime::parse(&s, &time::format_description::well_known::Rfc3339).map_err(
                |_| {
                    bad(format!(
                        "{what} is a time in RFC 3339, e.g. 2026-10-21T08:00:00Z"
                    ))
                },
            )
        })
        .transpose()
}

impl AuditFilter {
    /// Checks the filters and turns them into a query.
    pub(super) fn query(self) -> Result<AuditQuery, ApiError> {
        let outcome = text(self.outcome, "outcome")?;
        if outcome
            .as_deref()
            .is_some_and(|o| !matches!(o, "success" | "denied" | "failed"))
        {
            return Err(bad("outcome is success, denied or failed"));
        }
        let (from, to) = (time(self.from, "from")?, time(self.to, "to")?);
        if let (Some(f), Some(t)) = (from, to)
            && f >= t
        {
            return Err(bad("from must be before to"));
        }
        Ok(AuditQuery {
            actor: text(self.actor, "actor")?,
            target: text(self.target, "target")?,
            action_prefix: text(self.action, "action")?,
            outcome,
            from,
            to,
            before: self.before,
        })
    }

    /// The filters that were set, for the export's own audit row.
    fn describe(q: &AuditQuery) -> Value {
        let at = |t: Option<OffsetDateTime>| {
            t.and_then(|t| {
                t.format(&time::format_description::well_known::Rfc3339)
                    .ok()
            })
        };
        let mut o = serde_json::Map::new();
        for (k, v) in [
            ("actor", q.actor.clone()),
            ("target", q.target.clone()),
            ("action", q.action_prefix.clone()),
            ("outcome", q.outcome.clone()),
            ("from", at(q.from)),
            ("to", at(q.to)),
        ] {
            if let Some(v) = v {
                o.insert(k.into(), json!(v));
            }
        }
        if let Some(b) = q.before {
            o.insert("before".into(), json!(b));
        }
        Value::Object(o)
    }
}

/// The reader's filter for `audit.read`, or 403; and the database, or 503.
fn reader(
    state: &AppState,
    p: &Principal,
) -> Result<(AccessFilter, apelora_persistence::Pool), ApiError> {
    let filter = filter_for(p, "audit.read");
    if filter.is_empty() {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            "you may not read the audit log (audit.read)",
        ));
    }
    let Auth::Tokens(pool) = &state.auth else {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "database_not_configured",
            "the audit log needs a database (DATABASE_URL)",
        ));
    };
    Ok((filter, pool.clone()))
}

/// One page of the audit events the caller may read, newest first.
pub(super) async fn page(
    state: &AppState,
    p: &Principal,
    f: AuditFilter,
) -> Result<AuditList, ApiError> {
    let (filter, pool) = reader(state, p)?;
    let limit = f.limit.unwrap_or(100);
    if !(1..=500).contains(&limit) {
        return Err(bad("limit is 1 to 500"));
    }
    let q = f.query()?;
    let items = audit_history(&pool, &filter, &q, limit)
        .await?
        .into_iter()
        .map(AuditItem::from_row)
        .collect();
    Ok(AuditList { items })
}

/// Audit events the caller may read, newest first, filtered by actor, target, action prefix,
/// outcome and time, and paged with `before`. Needs `audit.read` (platform-wide, or limited to
/// the events of the caller's organisations, projects and applications) and a database.
#[utoipa::path(get, path = "/api/v1/audit", tag = "audit", operation_id = "audit", params(AuditFilter),
    responses((status = 200, body = AuditList), (status = 400, body = ErrorBody), (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn list(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Query(f): Query<AuditFilter>,
) -> Result<Json<AuditList>, ApiError> {
    Ok(Json(page(&state, &p, f).await?))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Format {
    Csv,
    JsonLines,
}

/// Spreadsheet programs run a cell that starts with `=`, `+`, `-`, `@`, a tab or a carriage
/// return as a formula, and some skip leading white space first: a value that starts with a tab
/// or carriage return, or whose first character after white space is one of those signs, is
/// prefixed with `'` so it stays text.
fn csv_field(v: &str) -> String {
    let formula = v.starts_with(['\t', '\r']) || v.trim_start().starts_with(['=', '+', '-', '@']);
    let v = if formula {
        format!("'{v}")
    } else {
        v.to_owned()
    };
    if v.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", v.replace('"', "\"\""))
    } else {
        v
    }
}

const CSV_HEADER: &str =
    "id,at,actor,actorName,actorType,action,target,outcome,requestId,scope,detail\r\n";

fn csv_line(r: &AuditRow) -> String {
    let at =
        r.at.format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default();
    let request = r.detail["requestId"].as_str().unwrap_or_default();
    let scope = r.scope.as_ref().map(Value::to_string).unwrap_or_default();
    let fields = [
        r.id.to_string(),
        at,
        r.actor.clone(),
        r.actor_name.clone().unwrap_or_default(),
        r.actor_type.clone(),
        r.action.clone(),
        r.target.clone(),
        r.outcome.clone(),
        request.to_owned(),
        scope,
        r.detail.to_string(),
    ];
    let mut line = fields
        .iter()
        .map(|f| csv_field(f))
        .collect::<Vec<_>>()
        .join(",");
    line.push_str("\r\n");
    line
}

fn json_line(r: AuditRow) -> String {
    let mut line = serde_json::to_string(&AuditItem::from_row(r)).unwrap_or_default();
    line.push('\n');
    line
}

struct Cursor {
    pool: apelora_persistence::Pool,
    filter: AccessFilter,
    query: AuditQuery,
    format: Format,
    started: bool,
    done: bool,
}

/// The next chunk of the export: the CSV header first, then one database page at a time, so a
/// large export never sits in memory.
async fn next_chunk(mut c: Cursor) -> Option<(Result<Bytes, std::io::Error>, Cursor)> {
    if c.done {
        return None;
    }
    if !c.started {
        c.started = true;
        if c.format == Format::Csv {
            return Some((Ok(Bytes::from_static(CSV_HEADER.as_bytes())), c));
        }
    }
    match audit_history(&c.pool, &c.filter, &c.query, EXPORT_PAGE).await {
        Err(e) => {
            tracing::error!(error = %e, "audit: export stopped by a database error");
            c.done = true;
            Some((Err(std::io::Error::other("audit export stopped")), c))
        }
        Ok(rows) if rows.is_empty() => None,
        Ok(rows) => {
            c.done = (rows.len() as i64) < EXPORT_PAGE;
            c.query.before = rows.last().map(|r| r.id);
            let out: String = rows
                .into_iter()
                .map(|r| match c.format {
                    Format::Csv => csv_line(&r),
                    Format::JsonLines => json_line(r),
                })
                .collect();
            Some((Ok(Bytes::from(out)), c))
        }
    }
}

/// Exports the audit events the caller may read, with the same filters as the list (`limit`
/// does not apply), newest first, as CSV or JSON lines. Streamed one database page at a time.
/// Needs `audit.read`; the export itself is audited as `audit.exported` with its filters.
#[utoipa::path(get, path = "/api/v1/audit/export", tag = "audit", operation_id = "audit_export",
    params(AuditFilter, ExportFormat),
    responses((status = 200, description = "The events: CSV with a header row, or one JSON object per line",
            content((String = "text/csv"), (String = "application/x-ndjson"))),
        (status = 400, body = ErrorBody), (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn export(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Query(f): Query<AuditFilter>,
    Query(fmt): Query<ExportFormat>,
) -> Result<Response, ApiError> {
    let (filter, pool) = reader(&state, &p)?;
    let format = match fmt.format.as_deref().map(str::trim) {
        None | Some("") | Some("jsonl") => Format::JsonLines,
        Some("csv") => Format::Csv,
        Some(_) => return Err(bad("format is csv or jsonl")),
    };
    let query = f.query()?;
    let (ext, content_type) = match format {
        Format::Csv => ("csv", "text/csv; charset=utf-8"),
        Format::JsonLines => ("jsonl", "application/x-ndjson"),
    };
    let mut detail = json!({ "format": ext, "filters": AuditFilter::describe(&query) });
    if let AccessFilter::Limited { .. } = filter {
        detail["limitedToScopes"] = json!(true);
    }
    // On the record before any event leaves: an export that cannot be audited does not happen.
    audit_alone(
        &pool,
        &p,
        "audit.exported",
        "audit",
        "success",
        None,
        &detail,
    )
    .await?;
    let cursor = Cursor {
        pool,
        filter,
        query,
        format,
        started: false,
        done: false,
    };
    let stamp = OffsetDateTime::now_utc();
    let name = format!(
        "apelora-audit-{:04}{:02}{:02}T{:02}{:02}{:02}Z.{ext}",
        stamp.year(),
        u8::from(stamp.month()),
        stamp.day(),
        stamp.hour(),
        stamp.minute(),
        stamp.second()
    );
    let mut res =
        Body::from_stream(futures_util::stream::unfold(cursor, next_chunk)).into_response();
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"{name}\"")) {
        h.insert(header::CONTENT_DISPOSITION, v);
    }
    Ok(res)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_quotes_what_needs_it_and_keeps_formulas_as_text() {
        assert_eq!(csv_field("plain"), "plain");
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_field("two\nlines"), "\"two\nlines\"");
        assert_eq!(csv_field("=cmd()"), "'=cmd()");
        assert_eq!(csv_field("-1+1"), "'-1+1");
        assert_eq!(csv_field("@x"), "'@x");
        assert_eq!(csv_field("=a,b"), "\"'=a,b\"");
        // Leading white space does not hide a formula from a spreadsheet.
        assert_eq!(csv_field(" =cmd()"), "' =cmd()");
        assert_eq!(csv_field("  +1"), "'  +1");
        assert_eq!(csv_field("\u{a0}@x"), "'\u{a0}@x");
        assert_eq!(csv_field("\t-1"), "'\t-1");
        assert_eq!(csv_field("\n=cmd()"), "\"'\n=cmd()\"");
        assert_eq!(csv_field("\r\n=x"), "\"'\r\n=x\"");
        assert_eq!(csv_field("\tplain"), "'\tplain");
        // A sign or @ later in the value is just text.
        assert_eq!(csv_field("a=b"), "a=b");
        assert_eq!(csv_field(" plain"), " plain");
        assert_eq!(csv_field(""), "");
    }

    #[test]
    fn filters_are_checked() {
        let q = |f: AuditFilter| f.query().map_err(|e| e.message().to_owned());
        assert!(q(AuditFilter::default()).is_ok());
        assert!(
            q(AuditFilter {
                outcome: Some("maybe".into()),
                ..Default::default()
            })
            .is_err()
        );
        assert!(
            q(AuditFilter {
                from: Some("yesterday".into()),
                ..Default::default()
            })
            .is_err()
        );
        assert!(
            q(AuditFilter {
                from: Some("2026-10-21T10:00:00Z".into()),
                to: Some("2026-10-21T09:00:00Z".into()),
                ..Default::default()
            })
            .is_err(),
            "an empty range is a mistake"
        );
        assert!(
            q(AuditFilter {
                actor: Some("x".repeat(201)),
                ..Default::default()
            })
            .is_err()
        );
        assert!(
            q(AuditFilter {
                target: Some("a\u{7}b".into()),
                ..Default::default()
            })
            .is_err()
        );
        let ok = q(AuditFilter {
            actor: Some("  ada ".into()),
            action: Some("".into()),
            from: Some("2026-10-21T08:00:00+02:00".into()),
            ..Default::default()
        })
        .expect("ok");
        assert_eq!(ok.actor.as_deref(), Some("ada"));
        assert_eq!(ok.action_prefix, None, "empty is no filter");
        assert!(ok.from.is_some());
    }
}
