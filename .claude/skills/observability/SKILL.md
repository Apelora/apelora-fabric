---
name: observability
description: Add structured logging, metrics, tracing, health checks, alerting, runbooks, and CI/CD automation to Apelora Fabric components, and define SLOs. Use when building any service, reconciler, or agent, when diagnosing behavior, or when preparing a component for release.
---

# Observability and automation

A component is not done until you can tell, from outside, whether it is healthy and why it is not. Build this in with the component, not afterward.

**Inputs:** the component and its failure modes. **Outputs:** instrumentation in code, dashboards or documented queries, alerts, and runbooks.

## Logging

- **Structured JSON**, one event per line. Required fields: `ts`, `level`, `component`, `msg`, `request_id`; add `resource_id`, `node_id`, `job_id`, `tenant_id`, `actor` where relevant.
- Log **state transitions and decisions** (job started/finished, reconcile chose action X because Y, node connected/disconnected, auth failure), not every function call.
- Levels: `error` = needs a human; `warn` = degraded but handled; `info` = significant transitions; `debug` = off by default.
- **Never log** secrets, tokens, passwords, private keys, full Xen domain configs, or full request bodies of imports. Redact by default; test redaction.
- Rust: `tracing` + `tracing-subscriber` JSON layer, spans per request and per reconcile cycle. TypeScript: a small logger wrapper; no stray `console.log` in committed code.
- Aggregate logs centrally; keep retention aligned with the capacity estimate and privacy needs.

## Metrics

Measure at three levels:

| Level | Examples |
|---|---|
| Host/process | CPU, memory, disk I/O, file descriptors, open connections |
| Component | API rate/errors/duration (RED), queue depth and oldest age, reconcile duration and backlog, node heartbeat age, cache hit rate, replication lag |
| Product/business | assemblies deployed, deploy success rate, time to running, active users |

- Use the **RED** method for services (Rate, Errors, Duration) and **USE** for resources (Utilization, Saturation, Errors).
- Histograms for latency (not averages); label cardinality kept low (never label by user id or resource id).
- Expose Prometheus-format metrics on a separate admin port from the public API.

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
