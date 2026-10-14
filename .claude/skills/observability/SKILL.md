---
name: observability
description: Add logging, metrics, tracing, health checks, and automation (CI/CD) to Apelora Fabric components. Use when building any service, reconciler, or agent, or when diagnosing production-style behavior.
---

# Observability and automation

## Logging

- Structured logs (JSON) with `request_id`, `resource_id`, `node_id`, and `job_id`; never log secrets, tokens, or full domain configs.
- One log line per significant state transition (job started/finished, reconcile decision, node connect/disconnect).
- Rust: `tracing` with spans per request and per reconcile cycle. TypeScript: a thin logger wrapper, no `console.log` in committed code.

## Metrics

Collect at three levels:
- **Host/process**: CPU, memory, disk I/O, open connections
- **Component**: API request rate/latency/errors, queue depth, reconcile duration and backlog, node heartbeat age
- **Product**: assemblies deployed, deploy success rate, time to running

Expose Prometheus-style metrics on a separate port from the public API.

## Health and alerts

- `/healthz` (process alive) and `/readyz` (dependencies reachable) on every service
- Alert on symptoms users feel (failed deploys, stale nodes), not only on resource thresholds
- Each alert links to a runbook entry in `docs/runbooks/`

## Automation

- CI on every push: format, lint, unit tests, license check, dependency audit (see `quality-assurance`, `license-compliance`)
- Automated build, test and release; releases are reproducible and tagged
- Deployments are scripted and identical across sites

## Checklist

- New component emits logs, metrics, and health endpoints before it is called done
- Dashboards or documented queries for its key metrics
- No sensitive data in telemetry
