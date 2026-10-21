---
name: observability
description: Add structured logging, metrics, tracing, health checks, alerting, runbooks, and CI/CD automation to Apelora Fabric components, and define SLOs. Use when building any service, reconciler, or agent, when diagnosing behavior, or when preparing a component for release.
---

# Observability and automation

A component is not done until you can tell, from outside, whether it is healthy and why it is not. Build this in with the component, not afterward.

**Inputs:** the component and its failure modes. **Outputs:** instrumentation in code, dashboards or documented queries, alerts, and runbooks.

## Logging in Apelora today (conventions; the runbook is `docs/runbooks/troubleshooting.md`)

- Both binaries share one setup (`init_logging` in each `main.rs`): `RUST_LOG` filters (default `info`), `APELORA_LOG_FORMAT=json` for JSON lines. Libraries use `tracing` only and never install a subscriber.
- **Fields**, always structured, never formatted into the message: `job`, `operation` (Incus or application operation id), `instance` as `project/name`, `application`, `module`, `member`, `volume`, `provider`, `issuer`, `code` (the API error code), `error = %e`. Messages are lowercase plain language with a subsystem prefix: `supervisor: …`, `move: …`, `job: …`, `incus: …`, `operation: …`, `network: …`, `audit: …`, `store: …`, `sign-in: …`.
- **Levels:** `error` = a loop-level failure, data at risk, or an audit write that failed; `warn` = degraded but continuing (a failed attempt that will be retried, a refusal by permission, an unreachable dependency); `info` = lifecycle (job claimed/succeeded, phase started, provider accepted); `debug` = one line per decision with its identifiers (plans, lifecycle steps, supervisor skips, each Incus request with status and latency, each successful API request).
- **Spans:** the control plane's outermost layer (`api/requests.rs`) gives each request an `x-request-id` (a safe caller value or a UUID), a `request` span (`request_id`, `method`, `path`, never the query) and one access line; audit events written during the request get `requestId` in their detail through `audit_scoped`. The worker runs each job in a `job` span (`job`, `kind`, `target`, `request_id` of the request that queued it, `-` for supervisor and scheduler jobs). Spans are error-level so their fields survive `RUST_LOG=warn`.
- **Floods:** a condition that can repeat per request (a rate-limit refusal) is logged at warn once when it starts (`rate limit: refusing requests limit=… client=…`) and at debug afterwards. The public `/health` is a cached yes/no (`api/health_cache.rs`); the reason Incus is unreachable is logged once per change (`health: Incus is not reachable`) and shown on the signed-in `/system`.
- **Correlation chain (ADR 0011 §2.8):** request → job → Incus operation. An API handler that queues a job uses `enqueue_job_for_request(…, requests::current().as_deref())` (or sets `NewOperation::request_id`), never plain `enqueue_job`; jobs a job queues inherit `job.request_id`. A job's Incus operations are recorded with `record_operation` (fenced) before waiting and kept in `jobs.incus_operations`; log the operation id at info inside the job span (`started operation operation=…`, `move: waiting on the Incus operation`). `GET /api/v1/jobs[/{id}]` returns `requestId` and `incusOperations`; the Console's Operations view shows the request id. Gap: operations the adapter waits on internally (volume copies, backups) are logged at debug only, not listed on the job.
- **Leadership and wake-up lines:** `leader: lease acquired|lost` (info), `leader: cannot renew the lease` (warn); `job wake-up: …` warns when the `LISTEN` connection is refused or lost and logs `listening for queued jobs again` at info once back.
- A condition that lasts across passes (a missing stateful instance, an unreadable member list) is logged when it starts or changes, then at most every 10 minutes (`Supervisor::should_report`).
- **Logged vs audited:** the audit trail records decisions about people's actions (changes, 403 refusals with their `code`, spent/mismatched approvals as `access.denied`); logs explain why. 401s and `approval_required` are logged, not audited (anyone could flood the trail; an approval request is normal flow). An audit write that must not fail the request goes through `authz::audit_failed`, never `let _ =`.
- Errors that reach clients name a next step and never carry raw database text (`store_error` names the request id; the log line has the detail). Incus adapter errors name the request (`GET /1.0/…`, query limited to naming parameters by `client::loggable`) and the HTTP status.
- **Never log** tokens of any kind, join secrets, certificates or keys, passwords, the `Authorization` header or request bodies; log the credential *kind*. Values from an unverified token are passed through `apelora_identity`'s `shown` (no control characters, bounded).

## Logging (target practice)

- **Structured JSON**, one event per line. Required fields: `ts`, `level`, `component`, `msg`, `request_id`; add `resource_id`, `node_id`, `job_id`, `tenant_id`, `actor` where relevant.
- Log **state transitions and decisions** (job started/finished, reconcile chose action X because Y, node connected/disconnected, auth failure), not every function call.
- Levels: `error` = needs a human; `warn` = degraded but handled; `info` = significant transitions; `debug` = off by default.
- **Never log** secrets, tokens, passwords, private keys, Incus client certificates or full instance configs, or full request bodies of imports. Redact by default; test redaction.
- Rust: `tracing` + `tracing-subscriber` JSON layer, spans per request and per reconcile cycle. TypeScript: a small logger wrapper; no stray `console.log` in committed code.
- Aggregate logs centrally; keep retention aligned with the capacity estimate and privacy needs.

## Metrics in Apelora today (design: `docs/architecture/MONITORING.md`)

- **One registry, no dependencies:** `crates/metrics` (`apelora-metrics`). Every family is a `static` in `crates/metrics/src/lib.rs` and listed in `FAMILIES`; `render()` writes the text exposition format (OpenMetrics when the collector's `Accept` asks for it). Constant labels `region`, `site`, `sector` (`APELORA_REGION/SITE/SECTOR`, validated, default `default`) are added to every series at render time.
- **Endpoints:** control plane `GET /metrics` behind `fabric.metrics.read` (gate in `GATES`, built-in role Metrics Reader for the collector's service identity); optional unauthenticated listener `APELORA_METRICS_ADDR` (off by default); worker listener `APELORA_WORKER_METRICS_ADDR` (default `127.0.0.1:9471`, `/metrics` and `/healthz` only). `GET /api/v1/monitoring` feeds the Console's Administration → Monitoring panel.
- **Adding a metric:**
  1. Declare it in `crates/metrics/src/lib.rs` (name `apelora_<area>_<what>`, unit suffix `_seconds`/`_bytes`, counters without `_total`; the writer adds it), and add it to `FAMILIES`.
  2. Record it at a **choke point** only: request middleware (`api/requests.rs`), authentication (`api.rs`), `authz::forbidden`, the job runner (`deployment-worker/src/lib.rs`), the supervisor loop, the storage schedule, the Incus client (`incus-adapter/src/client.rs::call`), or a scrape-time reader (`api/metrics.rs`, `persistence/src/monitoring.rs`). Never per request against PostgreSQL or Incus: scrape-time reads are cached (15 s database, 30 s inventory).
  3. Add a test that the family appears with the expected labels (`api/metrics_tests.rs`, `deployment-worker/src/metrics.rs`).
- **Cardinality rules:**
  - Label values come from small fixed sets: route templates (`MatchedPath`, never the URI), Incus endpoint templates (`endpoint_template`), status classes (`status_class`), outcome classes, catalogue permission ids (others become `other`), and job kinds (`job_kind_label`).
  - Never use a name, id, path, user, email address, token, certificate or error text as a label value.
  - A new label *name* must be added to the allow-list in the crate's test `no_family_is_labelled_by_an_identifier`. Add it only when its values are a small fixed set.
  - Each family keeps at most `MAX_SERIES` (1,000) label sets; extra ones are counted under `_overflow`. Seeing `_overflow` is a bug to fix.
- **Whole-state gauges** (members by status, jobs by kind) use `Gauge::replace`, so a state that disappears is not kept at its last value.
- **Lower layers** (Incus `/1.0/metrics` with a metrics-only certificate, PostgreSQL statistics views, OVN cluster status, SAN paths) are collected directly by the operator's collector. Do not proxy them through Apelora.
- **Several API processes** expose the same queue and inventory gauges: aggregate them with `max`.

## Metrics (target practice)

Measure at three levels:

| Level | Examples |
|---|---|
| Host/process | CPU, memory, disk I/O, file descriptors, open connections |
| Component | API rate/errors/duration (RED), queue depth and oldest age, reconcile duration and backlog, node heartbeat age, cache hit rate, replication lag |
| Product/business | assemblies deployed, deploy success rate, time to running, active users |

- Use the **RED** method for services (Rate, Errors, Duration) and **USE** for resources (Utilization, Saturation, Errors).
- Histograms for latency (not averages); label cardinality kept low (never label by user id or resource id).
- Expose metrics in the standard text exposition format; authenticated on the API address by default, an unauthenticated listener only on loopback or a management network.

## Tracing

- Propagate a trace/request id from Studio through Controller to Node (W3C `traceparent`). Use OpenTelemetry in Rust and the browser.
- Sample sensibly (always trace errors and slow requests).

## Health, SLOs, alerts

- `/healthz` = process alive; `/readyz` = dependencies reachable and ready for traffic. Orchestrators use both.
- Define SLOs per user-visible journey (e.g. "99% of deploys reach running within 5 min"), with an error budget.
- Alert on **symptoms users feel** (failed deploys, stale nodes, SLO burn rate); use resource alerts only as supporting signals. Every alert has an owner and a runbook.
- Runbooks in `docs/runbooks/<alert>.md`: meaning, impact, first checks, mitigation, escalation.

## Automation (CI/CD)

- On every push and PR: format, lint, type-check, unit and component tests, Rust `fmt/clippy/test`, license check, dependency audit, secret scan, dependency review.
- On merge: build, test, SBOM, tag, publish artifacts reproducibly; signed releases.
- Infrastructure and deployment as code; the same scripts for every site.
- Pin tool and action versions; keep builds hermetic where possible.

## Checklist before calling a component done

- [ ] Structured logs with request/resource ids; secrets redaction tested
- [ ] RED/USE metrics exposed; queue depth and lag measured if applicable
- [ ] `/healthz` and `/readyz` implemented and tested
- [ ] At least one SLO and one symptom alert with a runbook
- [ ] Traces connect across Studio → Controller → Node
- [ ] CI covers the component
- [ ] No sensitive data in telemetry

## Acceptance criteria

A reviewer can answer from telemetry alone: Is it up? Is it slow? Is it failing, for whom, and why? What changed recently?
