// SPDX-License-Identifier: AGPL-3.0-only
//! Process logs and optional traces for the control plane and the worker.
//!
//! `init` installs the subscriber both binaries used to copy: `RUST_LOG` (default `info`) and
//! `APELORA_LOG_FORMAT` (`text` or `json`). When `APELORA_OTLP_ENDPOINT` is a URL, completed
//! `request` and `job` spans are posted as OTLP/HTTP JSON to `{endpoint}/v1/traces`. Off by
//! default; the collector still *pulls* metrics. Traces are offered only to the receiver the
//! operator names, typically a Tempo on loopback inside the site (`infra/observe`).

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};
use tracing::Subscriber;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{Layer, fmt};

/// Install logging, and trace export when `APELORA_OTLP_ENDPOINT` is set.
///
/// `service` is the OTLP `service.name` (`apelora-control-plane` or `apelora-deployment-worker`).
/// Call from a Tokio runtime (`#[tokio::main]`); without one, traces are not exported.
pub fn init(service: &'static str) {
    let filter = match std::env::var("RUST_LOG") {
        Ok(v) if !v.trim().is_empty() => EnvFilter::try_new(&v).unwrap_or_else(|e| {
            eprintln!("RUST_LOG={v:?} is not a valid filter ({e}); logging at info");
            EnvFilter::new("info")
        }),
        _ => EnvFilter::new("info"),
    };
    let json = match std::env::var("APELORA_LOG_FORMAT").as_deref() {
        Ok("json") => true,
        Ok("" | "text") | Err(_) => false,
        Ok(other) => {
            eprintln!("APELORA_LOG_FORMAT must be text or json; writing text ({other})");
            false
        }
    };
    let otlp = OtlpLayer::from_env(service);
    let registry = tracing_subscriber::registry().with(filter);
    if json {
        registry
            .with(fmt::layer().json().with_span_list(false))
            .with(otlp)
            .init();
    } else {
        registry.with(fmt::layer()).with(otlp).init();
    }
}

struct OtlpLayer {
    service: &'static str,
    tx: tokio::sync::mpsc::Sender<String>,
}

/// Parsed `APELORA_OTLP_ENDPOINT`. Empty and `off` mean traces stay off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TracesEndpoint {
    pub url: String,
    /// True when the host is loopback. Non-loopback still exports, with a warning: the collector
    /// belongs inside the installation.
    pub loopback: bool,
}

fn is_loopback_host(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host.parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_loopback())
}

/// Empty and the documented `off` value. Both also fail `Url::parse`; kept so those values stay
/// explicit for operators. Equivalent for mutation testing.
fn traces_disabled(raw: &str) -> bool {
    let endpoint = raw.trim();
    endpoint.is_empty() || endpoint == "off"
}

/// Parse the operator's OTLP base URL. Appends `/v1/traces` when the path does not already end
/// with it. Only `http` and `https` are accepted.
pub fn traces_endpoint(raw: &str) -> Option<TracesEndpoint> {
    if traces_disabled(raw) {
        return None;
    }
    let endpoint = raw.trim();
    match reqwest::Url::parse(endpoint) {
        Ok(mut u) if u.scheme() == "http" || u.scheme() == "https" => {
            if !u.path().ends_with("/v1/traces") {
                let path = u.path().trim_end_matches('/');
                u.set_path(&format!("{path}/v1/traces"));
            }
            let host = u.host_str().unwrap_or("");
            Some(TracesEndpoint {
                url: u.to_string(),
                loopback: is_loopback_host(host),
            })
        }
        _ => None,
    }
}

impl OtlpLayer {
    fn from_env(service: &'static str) -> Option<Self> {
        let raw = std::env::var("APELORA_OTLP_ENDPOINT").ok()?;
        let parsed = traces_endpoint(&raw)?;
        if !parsed.loopback {
            eprintln!(
                "APELORA_OTLP_ENDPOINT is not loopback; keep the receiver inside this installation"
            );
        }
        let url = parsed.url;
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            eprintln!(
                "APELORA_OTLP_ENDPOINT is set but there is no Tokio runtime; traces are not exported"
            );
            return None;
        };
        let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(256);
        handle.spawn(async move {
            let client = reqwest::Client::new();
            while let Some(body) = rx.recv().await {
                match client
                    .post(&url)
                    .header("content-type", "application/json")
                    .body(body)
                    .send()
                    .await
                {
                    Ok(res) if res.status().is_success() => {}
                    Ok(res) => tracing::debug!(
                        status = res.status().as_u16(),
                        "otlp: the trace receiver refused a span"
                    ),
                    Err(e) => tracing::debug!(error = %e, "otlp: could not post a span"),
                }
            }
        });
        Some(Self { service, tx })
    }
}

struct SpanData {
    name: &'static str,
    fields: HashMap<&'static str, String>,
    start: SystemTime,
}

impl Visit for SpanData {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.fields.insert(field.name(), format!("{value:?}"));
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        self.fields.insert(field.name(), value.to_owned());
    }
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.fields.insert(field.name(), value.to_string());
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        self.fields.insert(field.name(), value.to_string());
    }
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.fields.insert(field.name(), value.to_string());
    }
}

impl<S> Layer<S> for OtlpLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let name = attrs.metadata().name();
        if name != "request" && name != "job" {
            return;
        }
        let Some(span) = ctx.span(id) else {
            return;
        };
        let mut data = SpanData {
            name,
            fields: HashMap::new(),
            start: SystemTime::now(),
        };
        attrs.record(&mut data);
        span.extensions_mut().insert(data);
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else {
            return;
        };
        let mut ext = span.extensions_mut();
        if let Some(data) = ext.get_mut::<SpanData>() {
            values.record(data);
        }
    }

    fn on_close(&self, id: Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(&id) else {
            return;
        };
        let Some(data) = span.extensions_mut().remove::<SpanData>() else {
            return;
        };
        let body = encode_span(self.service, &data, SystemTime::now());
        if self.tx.try_send(body).is_err() {
            tracing::debug!("otlp: dropping a span; the export queue is full");
        }
    }
}

/// OTLP/HTTP JSON for one `request` or `job` span. Same `request_id` (when not `-`) hashes to the
/// same trace id in both processes, so a job the API queued shows on the same trace.
pub fn encode_span(service: &str, data: &impl SpanFields, end: SystemTime) -> String {
    let request_id = data.field("request_id");
    let request_id = request_id.as_deref().unwrap_or("-");
    let job = data.field("job");
    let seed = if request_id != "-" {
        request_id
    } else {
        job.as_deref().unwrap_or(data.name())
    };
    let trace_id = hex(&Sha256::digest(seed.as_bytes())[..16]);
    let mut span_seed = seed.to_owned();
    span_seed.push_str(data.name());
    span_seed.push_str(&nanos(data.start()).to_string());
    let span_id = hex(&Sha256::digest(span_seed.as_bytes())[..8]);
    let mut attrs = vec![otlp_str("service.name", service)];
    if request_id != "-" {
        attrs.push(otlp_str("request_id", request_id));
    }
    for key in ["method", "path", "kind", "target", "job"] {
        if let Some(v) = data.field(key) {
            attrs.push(otlp_str(key, &v));
        }
    }
    serde_json::json!({
        "resourceSpans": [{
            "resource": {
                "attributes": [otlp_str("service.name", service)]
            },
            "scopeSpans": [{
                "scope": { "name": "apelora" },
                "spans": [{
                    "traceId": trace_id,
                    "spanId": span_id,
                    "name": data.name(),
                    "kind": 2,
                    "startTimeUnixNano": nanos(data.start()).to_string(),
                    "endTimeUnixNano": nanos(end).to_string(),
                    "attributes": attrs,
                }]
            }]
        }]
    })
    .to_string()
}

fn otlp_str(key: &str, value: &str) -> serde_json::Value {
    serde_json::json!({ "key": key, "value": { "stringValue": value } })
}

fn nanos(t: SystemTime) -> u128 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut s, b| {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// Fields a finished span exposes to [`encode_span`]. Tests build a stub; the layer uses `SpanData`.
pub trait SpanFields {
    fn name(&self) -> &str;
    fn start(&self) -> SystemTime;
    fn field(&self, key: &str) -> Option<String>;
}

impl SpanFields for SpanData {
    fn name(&self) -> &str {
        self.name
    }
    fn start(&self) -> SystemTime {
        self.start
    }
    fn field(&self, key: &str) -> Option<String> {
        self.fields.get(key).cloned().map(unquote)
    }
}

/// `record_debug` wraps strings in quotes; OTLP attributes should not keep them.
pub fn unquote(s: String) -> String {
    s.strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .map(ToOwned::to_owned)
        .unwrap_or(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    struct Stub {
        name: &'static str,
        start: SystemTime,
        fields: HashMap<&'static str, String>,
    }
    impl SpanFields for Stub {
        fn name(&self) -> &str {
            self.name
        }
        fn start(&self) -> SystemTime {
            self.start
        }
        fn field(&self, key: &str) -> Option<String> {
            self.fields.get(key).cloned()
        }
    }

    #[test]
    fn request_spans_share_a_trace_id_from_the_request_id() {
        let start = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let end = start + Duration::from_millis(12);
        let a = Stub {
            name: "request",
            start,
            fields: HashMap::from([
                ("request_id", "req-1".into()),
                ("method", "GET".into()),
                ("path", "/api/v1/health".into()),
            ]),
        };
        let b = Stub {
            name: "job",
            start,
            fields: HashMap::from([
                ("request_id", "req-1".into()),
                ("kind", "instance.create".into()),
                ("job", "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".into()),
            ]),
        };
        let ja: serde_json::Value =
            serde_json::from_str(&encode_span("apelora-control-plane", &a, end)).expect("json");
        let jb: serde_json::Value =
            serde_json::from_str(&encode_span("apelora-deployment-worker", &b, end)).expect("json");
        let span_a = &ja["resourceSpans"][0]["scopeSpans"][0]["spans"][0];
        let span_b = &jb["resourceSpans"][0]["scopeSpans"][0]["spans"][0];
        assert_eq!(span_a["traceId"], span_b["traceId"]);
        assert_ne!(span_a["spanId"], span_b["spanId"]);
        assert_eq!(span_a["traceId"].as_str().expect("trace id").len(), 32);
        assert_eq!(span_a["spanId"].as_str().expect("span id").len(), 16);
        assert_eq!(span_a["name"], "request");
        assert_eq!(span_b["name"], "job");
        let body = ja.to_string();
        assert!(!body.contains("Bearer"));
        assert!(!body.contains("apl_"));
        assert!(body.contains("apelora-control-plane"));
        assert!(body.contains("/api/v1/health"));
    }

    #[test]
    fn supervisor_jobs_without_a_request_id_do_not_share_a_trace() {
        let start = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let end = start + Duration::from_millis(1);
        let a = Stub {
            name: "job",
            start,
            fields: HashMap::from([("request_id", "-".into()), ("job", "job-a".into())]),
        };
        let b = Stub {
            name: "job",
            start,
            fields: HashMap::from([("request_id", "-".into()), ("job", "job-b".into())]),
        };
        let ja: serde_json::Value =
            serde_json::from_str(&encode_span("worker", &a, end)).expect("json");
        let jb: serde_json::Value =
            serde_json::from_str(&encode_span("worker", &b, end)).expect("json");
        assert_ne!(
            ja["resourceSpans"][0]["scopeSpans"][0]["spans"][0]["traceId"],
            jb["resourceSpans"][0]["scopeSpans"][0]["spans"][0]["traceId"]
        );
    }

    #[test]
    fn a_missing_request_id_uses_the_job_id_as_the_trace_seed() {
        let start = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let end = start + Duration::from_millis(1);
        let a = Stub {
            name: "job",
            start,
            fields: HashMap::from([("job", "same-job".into())]),
        };
        let b = Stub {
            name: "job",
            start,
            fields: HashMap::from([("request_id", "-".into()), ("job", "same-job".into())]),
        };
        let ja: serde_json::Value =
            serde_json::from_str(&encode_span("worker", &a, end)).expect("json");
        let jb: serde_json::Value =
            serde_json::from_str(&encode_span("worker", &b, end)).expect("json");
        assert_eq!(
            ja["resourceSpans"][0]["scopeSpans"][0]["spans"][0]["traceId"],
            jb["resourceSpans"][0]["scopeSpans"][0]["spans"][0]["traceId"]
        );
    }

    #[test]
    fn otlp_payload_uses_server_span_kind_and_nanosecond_times() {
        let start = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let end = start + Duration::from_millis(12);
        let a = Stub {
            name: "request",
            start,
            fields: HashMap::from([
                ("request_id", "req-2".into()),
                ("method", "POST".into()),
                ("path", "/api/v1/jobs".into()),
            ]),
        };
        let v: serde_json::Value =
            serde_json::from_str(&encode_span("apelora-control-plane", &a, end)).expect("json");
        let span = &v["resourceSpans"][0]["scopeSpans"][0]["spans"][0];
        assert_eq!(span["kind"], 2);
        assert_eq!(span["startTimeUnixNano"], "1700000000000000000");
        assert_eq!(span["endTimeUnixNano"], "1700000000012000000");
        assert_eq!(
            v["resourceSpans"][0]["scopeSpans"][0]["scope"]["name"],
            "apelora"
        );
        let attrs = span["attributes"].as_array().expect("attrs");
        let keys: Vec<&str> = attrs
            .iter()
            .map(|a| a["key"].as_str().expect("key"))
            .collect();
        assert!(keys.contains(&"request_id"));
        assert!(keys.contains(&"method"));
        assert!(keys.contains(&"path"));
        assert!(!keys.contains(&"job"));
    }

    #[test]
    fn traces_endpoint_accepts_loopback_http_and_rejects_other_schemes() {
        let local = traces_endpoint("http://127.0.0.1:4318").expect("loopback");
        assert!(local.loopback);
        assert!(local.url.ends_with("/v1/traces"));
        assert!(!local.url.contains("/v1/traces/v1/traces"));
        let already = traces_endpoint("http://127.0.0.1:4318/v1/traces").expect("path kept");
        assert_eq!(already.url.matches("/v1/traces").count(), 1);
        assert!(
            traces_endpoint("https://localhost:4318")
                .expect("localhost")
                .loopback
        );
        assert!(traces_endpoint("http://[::1]:4318").expect("ipv6").loopback);
        assert!(
            traces_endpoint("http://127.0.0.2:4318")
                .expect("loopback net")
                .loopback
        );
        assert!(
            traces_endpoint("http://LocalHost:4318")
                .expect("case")
                .loopback
        );
        let remote = traces_endpoint("http://10.0.0.1:4318").expect("still exported");
        assert!(!remote.loopback);
        assert!(remote.url.ends_with("/v1/traces"));
        assert!(traces_endpoint("off").is_none());
        assert!(traces_endpoint("").is_none());
        assert!(traces_endpoint("   ").is_none());
        assert!(traces_endpoint("ftp://127.0.0.1:4318").is_none());
        assert!(traces_endpoint("not a url").is_none());
    }

    #[test]
    fn debug_quoted_field_values_lose_the_wrapping_quotes() {
        assert_eq!(unquote("\"req-1\"".into()), "req-1");
        assert_eq!(unquote("req-1".into()), "req-1");
        assert_eq!(unquote("\"still-open".into()), "\"still-open");
        assert_eq!(unquote("123".into()), "123");
    }
}
