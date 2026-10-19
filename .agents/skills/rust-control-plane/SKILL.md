---
name: rust-control-plane
description: Build the Rust control plane — Axum/Tokio REST API, deployment worker, desired-state reconciliation, durable jobs and leases in PostgreSQL, and the Cargo workspace layout. Use when working on services/, crates/, API endpoints, reconcilers or job coordination.
---

# Rust control plane

Apelora's backend is Rust. It owns application graphs, appliance definitions, desired state, replica policy and health; **Incus** owns cluster membership, placement and instance lifecycle (see `incus-platform`, ADR `docs/adr/0001-incus-first.md`).

## Layout

- Executables: `services/control-plane` (Axum REST API, `/api/v1`) and `services/deployment-worker` (durable reconciler). Nothing else is a separate service; the network fabric, coordination, placement, health and recovery code are **libraries** called by these two.
- Libraries under `crates/`: narrow and acyclic. Today: `domain` (types and validation, no I/O) and `incus-adapter` (typed Incus REST client). Planned, created when first needed: `persistence`, `auth`, `orchestration`, `network-fabric`, `service-discovery`, `coordination`, `placement`, `health`, `mobility`, `recovery`, `storage-policy`.
- Keep `domain` free of I/O. Adapters depend on `domain`, never the reverse.

## Architecture

- The API stores **desired state** and returns; it never performs long work inline. The worker converges **observed state** toward desired state.
- Keep Desired, Observed and LastKnown state separate, each with timestamp, source and operation correlation id.
- Reconcile loops are idempotent, level-triggered and safe to re-run after a crash. Use generation/revision numbers so a stale worker cannot commit an outdated decision.
- Jobs are persisted with explicit states (`pending|running|succeeded|failed|cancelled`), bounded retries with backoff, and per-resource ownership.
- Coordination lives in **PostgreSQL**: claim jobs with row locks (`FOR UPDATE SKIP LOCKED`) or durable leases with expiry; one reconciliation flow per application at a time. Database locks alone do not prevent split-brain against an external API — also use idempotency keys, Incus operation ids, ownership checks and fencing where required. Do not add a separate consensus/coordination service without a measured need and an ADR.
- Never hold a DB transaction while waiting on a long-running Incus operation: persist the job and Incus operation id, then poll/synchronise. After a restart, read actual Incus state before retrying.
- The browser never talks to Incus and never sees Incus credentials.

## Conventions

- `tokio` + `axum`; `serde`; SQLx with migrations; `reqwest` with TLS verification on; `tracing` with request, resource and operation ids.
- `thiserror` for library errors; typed API errors with stable codes; no `unwrap` outside tests.
- Verify the installed toolchain/MSRV and crate versions before choosing dependencies; pin with `Cargo.lock`.
- API changes are reflected in the OpenAPI document and the generated TypeScript client.
- `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` must pass.

## Checklist

- Reconciler tested with a fake provider: crash mid-operation, partial failure, duplicate delivery, worker restart
- Authn/authz on every route, scoped by project (see `security-tenancy`, `security-review`)
- Results labelled MOCK VERIFIED, CONTRACT VERIFIED or REAL INCUS TESTED (see `testing-operations`)
