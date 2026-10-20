---
name: rust-control-plane
description: Build the Rust control plane — Axum/Tokio REST API, deployment worker, desired-state reconciliation, durable jobs and leases in PostgreSQL, and the Cargo workspace layout. Use when working on services/, crates/, API endpoints, reconcilers or job coordination.
---

# Rust control plane

Apelora's backend is Rust. It owns application graphs, module definitions, desired state, replica policy and health; **Incus** owns cluster membership, placement and instance lifecycle (see `incus-platform`, ADR `docs/adr/0001-incus-first.md`).

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
- Every handler returns a typed `ToSchema` struct (never `serde_json::json!`) and carries `#[utoipa::path]` with its error statuses; register it in `ApiDoc` in `services/control-plane/src/api.rs`. Then run `npm run api:generate` and commit `packages/api-client/openapi.json` and `src/generated.ts`; `committed_openapi_document_is_current` and the CI generated-file check fail otherwise. Frontends call the API only through `@apelora/api-client`.
- `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` must pass.

## Checklist

- Reconciler tested with a fake provider: crash mid-operation, partial failure, duplicate delivery, worker restart
- Authn/authz on every route, scoped by project (see `security-tenancy`, `security-review`)
- Results labelled MOCK VERIFIED, CONTRACT VERIFIED or REAL INCUS TESTED (see `testing-operations`)

## Persistence and worker (Phase 03)

- `crates/persistence`: SQLx (runtime queries), embedded migrations in `crates/persistence/migrations` (`MIGRATOR`). Tables: `applications`, immutable `application_revisions` (numbered per application under a row lock), `jobs`, `audit_events`.
- Jobs: `enqueue_job` is idempotent by `idempotency_key`; `claim_job` uses `FOR UPDATE SKIP LOCKED` and also reclaims running jobs whose lease expired; every claim bumps `generation`, and `renew_lease`/`complete_job` are fenced on (owner, generation), so a worker that lost its lease gets `StoreError::Conflict` and its result is discarded. Failures (`Outcome::Failed`) retry with exponential backoff (max 300 s) until `max_attempts`, then `failed`; `Outcome::Rejected` fails at once when retrying cannot help. `record_operation` stores the Incus operation id on the job (fenced) before waiting; the next attempt sees it as `ClaimedJob::external_operation`.
- `services/deployment-worker`: `run_once`/`run` loop with lease renewal at a third of the lease; unknown job kinds fail rather than being ignored. Lifecycle handlers live in `services/deployment-worker/src/lifecycle.rs`: a pure `next_step(goal, observed)` decides each call from a fresh read of Incus, so retried, duplicated or reclaimed jobs converge. Instances without `MANAGED_KEY` (`user.apelora.managed=true`) are never changed. The worker reads the same `APELORA_INCUS_*` settings as the control plane (`apelora_incus_adapter::config`).
- Tests use `#[sqlx::test(migrator = ...)]` (fresh database per test). Locally `docker compose -f infra/dev/compose.yaml up -d`; `.cargo/config.toml` points `DATABASE_URL` at it unless the environment overrides it (CI uses a PostgreSQL service).

## Current API

Read: `GET /api/v1/health`, `/whoami` (role, permissions, `deployProjects`), `/jobs` (recent), `/system` (version, pinned Incus from `upstream.lock.yaml` at build time), `/cluster`, `/instances`, `/projects`, `/networks`, `/storage-pools`, `/preflight`, `/audit`, `/jobs/{id}`, `/openapi.json`. Preflight logic is pure (`apelora_domain::preflight::evaluate`); anything it cannot observe is `not-checked`, never `pass`. Write (`deploy` permission, Phase 04): `POST /api/v1/instances`, `POST /api/v1/instances/{project}/{name}/state`, `DELETE /api/v1/instances/{project}/{name}`. Writes never call Incus: they validate, check the project against `APELORA_DEPLOY_PROJECTS` (default `apelora`), audit `<kind>.requested`, enqueue a job (optional `Idempotency-Key`, scoped to the caller) and return `202 {jobId, created}`. Inventory responses carry `source: {provider: mock|incus, observedAt}`; adapter failures map to 502/503 with stable codes (`incus_unreachable`, `incus_authentication`, `incus_api_error`, `incus_bad_response`, `incus_unsupported_capability`, ...). `InstanceSpec`: `project`, `name`, `kind`, `source {protocol, server, alias, fingerprint?}`, `profiles` (empty = the project's `default`), `config` (`user.apelora.*` reserved), `start`, `architectures` (empty = any). The worker checks image pins and architectures before and after creating (`lifecycle::placeable`, `next_step`). Behaviour against Incus is proven with `services/deployment-worker/tests/real_incus.rs` on the disposable test cluster (`docs/development/TEST-CLUSTER.md`); a contract test alone does not make a feature REAL INCUS TESTED. Applications (`api/applications.rs`, design `docs/design/application-deployment.md`): `POST /api/v1/applications/{name}/plan` and `…/deployments` take the Studio document plus one `InstanceSpec` per deployable module (`DesiredModule`); the plan is always recomputed server-side with `apelora_domain::plan::plan` (create/keep/remove/conflict), a conflict refuses everything, specs get their name/project/owner from `plan::owned_spec` (labels `user.apelora.application`/`module`), revisions are immutable and idempotent per key (`add_revision_once`). `GET …/status` filters inventory by the labels. Cluster membership (ADR 0003, `api/cluster_admin.rs`, `manage-infrastructure`): `POST /api/v1/cluster/join-tokens {memberName}` returns a single-use token once (`Cache-Control: no-store`, never stored, logged or audited), `GET` lists pending joins without secrets, `DELETE …/{member}` revokes. These call Incus directly through `AppState.admin`, the optional unrestricted identity (`APELORA_INCUS_ADMIN_CLIENT_CERT`/`KEY`, `config::admin_provider_from_env`) that only the control plane loads; never give it to the worker and never queue anything that carries a secret. A 204 response is `null` in the generated client. `incus-adapter` exposes `Provider` (enum: `Mock` | `Incus`), not a trait object; its `test-util` feature exposes `IncusClient::plain_for_tests` for other crates' tests only.
