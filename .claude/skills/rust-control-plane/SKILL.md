---
name: rust-control-plane
description: Build the Rust Controller — HTTP API, desired-state reconciliation, job queue, scheduling and orchestration. Use when working on controller crates, API endpoints, reconcilers, or node coordination.
---

# Rust control plane

## Architecture

- API stores **desired state**; reconcilers converge **observed state** toward it. Endpoints never perform long work inline — they write intent and return.
- Reconcile loops are idempotent, level-triggered, and safe to re-run after a crash. Use generation/revision numbers and optimistic concurrency.
- Jobs: persisted, retryable with backoff, with explicit states (`pending|running|succeeded|failed|cancelled`) and per-resource locking.
- Scheduler: places instances on nodes by architecture, capacity, and anti-affinity; deterministic given the same inputs.
- Controller talks to nodes through a narrow, versioned, authenticated RPC; it never touches Xen directly (see `xen-integration`).

## Conventions

- Workspace crates: `api`, `core` (types, validation), `reconcile`, `store`, `scheduler`. Keep `core` free of I/O.
- `thiserror` for library errors, typed API errors with stable codes; no `unwrap` outside tests.
- `tokio` + `axum`; `tracing` with request and resource ids; `serde` types generated from the schemas.
- `cargo fmt`, `cargo clippy -- -D warnings`, `cargo test` must pass.

## Checklist

- Reconciler tested with a fake node (crash, partial failure, duplicate delivery)
- API changes reflected in the OpenAPI spec and Studio client
- Authn/authz applied on every route (see `security-review`)
