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

## Apelora specifics

- CI gates in place: `.github/workflows/ci.yml` (frontend, rust, e2e, dependencies: `cargo deny` + npm licences/audit) and `.github/workflows/upstream-check.yml` (weekly Incus pin verification and update PRs). Dependabot covers GitHub Actions, npm and Cargo weekly.
- Generated files (JSON Schemas, `docs/architecture/CATALOG.md`) are checked for drift in CI; regenerate with `npm run schema` / `npm run catalog:doc`.
- Real-Incus integration tests run only against the designated disposable cluster (four Lima VMs standing in for physical servers, plus a Lima VM `apelora-san` as the lab SAN; docs/development/TEST-CLUSTER.md), outside pull-request CI: `cargo test -p apelora-deployment-worker --test real_incus -- --ignored --test-threads 1`. End-to-end runs through the real control plane and worker use a short-lived token, and the token and the run's database rows are deleted afterwards.
- End-to-end demonstration: `bash scripts/test-cluster/demo.sh` (needs at least three online members; PASS/FAIL per check, log in a temp dir, cleans up even on failure). Run it after changes to the worker, supervisor or networking.
- Failure drills on the lab: `limactl stop -f apelora-incus-2` simulates a member crash (the cluster keeps quorum with three voters and a standby); `limactl start` brings it back. Feed scripts to `bash -s` with `</dev/null` on `incus launch`.
- Lab storage: pools `local` (btrfs, per member), `backup` (dir, `backup-pool.sh`) and `shared` (clustered LVM on the SAN VM's iSCSI LUN, `shared-storage.sh plan|apply|status`), used by `data-replicated` volumes. Fence probe: `fence-probe.sh plan|apply|status|remove`. HA slice B drill: `ha-drill.sh plan` (read-only, writes a plan file) then `ha-drill.sh run <planfile>`: own database, CP on :8095, one test application, one member VM force-stopped (never `apelora-san`, the API/OVN member or the leader; quorum checked), timeline, cleanup on exit.
- Runbooks live in `docs/runbooks/`; every step says REAL INCUS TESTED (with date and cluster size) or NOT YET VERIFIED. Update the label when a step is exercised; never upgrade it without doing the step.

