---
name: reconciliation-ha
description: Use for replicas, health checks, deployment worker, failure detection and failover decisions.
---

# Desired State and HA Reconciliation

## When to use

Use for replicas, health checks, deployment worker, failure detection and failover decisions.

## Required workflow and guardrails

- Define Desired, Observed and LastKnown states separately, with timestamps, source evidence and operation correlation.
- Classify node heartbeat, Incus instance state, app readiness, ingress health and storage integrity separately. Running != healthy.
- For stateless replicas: rollout gates, known stable IDs, bounded retry, distinct node preference, cooldown and healthy ingress update.
- Avoid duplicate mutations with durable per-resource lease/lock/fencing token and safe idempotency.
- For stateful single writers: block automated failover unless old writer is fenced, shared storage safe, data ownership exclusive and database plan verified.
- Incus `cluster.healing_threshold` is disabled by default; never enable automatically in new environments.
- Record audited timeline for failure detection, decision, request, operation completion and post-checks.
- Test worker crashes, stale probes, duplicate requests, network partitions, storage outages, recovery/failback.

## Evidence required before calling a feature complete

Explain what changed, actual tests and commands run, source/API reference, remaining safety constraints, and whether tested against real Incus or only mocks.

## Primary references

- https://linuxcontainers.org/incus/docs/main/
- https://github.com/lxc/incus
- https://doc.rust-lang.org/cargo/reference/workspaces.html
