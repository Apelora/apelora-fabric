---
name: testing-operations
description: Use for test evidence, mock vs real environments, monitoring, CI/CD and incident procedures.
---

# Testing, Observability and Operational Runbooks

## When to use

Use for test evidence, mock vs real environments, monitoring, CI/CD and incident procedures.

## Required workflow and guardrails

- Classify test outcomes: MOCK VERIFIED, CONTRACT VERIFIED, REAL INCUS TESTED, NOT TESTED/NOT IMPLEMENTED.
- Never call a simulated deployment production-ready. Maintain disposable Incus integration cluster with least privilege and explicit destructive test scope.
- Test quorum/member loss, process health, graceful stop vs crash, API delay, worker duplicate, storage failure, network partition, digest mismatch, secrets rotation.
- Never automatically fail over a stateful writer without fencing and exclusive access. Use safe, staged fault-injection labs.
- Incus `/1.0/metrics` exposes local member instance data; scrape each member, aggregate with labels, avoid excessive polling.
- Operations and events require correlation IDs, state transitions, error diagnostics, audit record and runbook link.
- CI gates: rustfmt/clippy/test, TypeScript tests/build, OpenAPI compatibility, security scan, upstream version checks.
- Runbooks always include prerequisites, preflight, execution, verification, rollback and evidence.

## Evidence required before calling a feature complete

Explain what changed, actual tests and commands run, source/API reference, remaining safety constraints, and whether tested against real Incus or only mocks.

## Primary references

- https://linuxcontainers.org/incus/docs/main/
- https://github.com/lxc/incus
- https://doc.rust-lang.org/cargo/reference/workspaces.html
