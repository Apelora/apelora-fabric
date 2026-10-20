# ADR 0003 — Two Incus identities; members join with single-use tokens

Status: **accepted** (Phase 04) · Date: 2026-10-20

## Context

Apelora reaches Incus over mutual TLS with a client certificate that Incus trusts. Deployments only need one project: Incus can restrict a trusted certificate to a list of projects, and such a certificate cannot see or change anything outside them. Cluster administration is different: issuing a join token for a new member (`POST /1.0/cluster/members`), listing or revoking tokens, and later evacuation and member removal are server-wide operations that Incus allows only to an unrestricted identity.

A join token is a secret. It is single-use, Incus cancels an older token for the same member name when a new one is issued, and it expires after `cluster.join_token_expiry` (3 hours by default). Whoever holds a valid token can add a server to the cluster.

Apelora's write requests normally go through durable jobs: the API validates, audits and queues; the worker calls Incus. A job's payload and outcome are stored in PostgreSQL.

## Decision

1. **Two identities.** The *deploy* identity (`APELORA_INCUS_CLIENT_CERT`/`KEY`) is restricted to the deploy projects and used by the control plane (inventory, preflight) and the deployment worker. The *admin* identity (`APELORA_INCUS_ADMIN_CLIENT_CERT`/`KEY`) is unrestricted, optional, and loaded **only by the control plane**; the worker never receives it. Without it, cluster administration answers `503 cluster_admin_not_configured` and everything else works.
2. **Approval is issuing a token.** A new member joins only after a fabric administrator (`manage-infrastructure`) asks for a token for its exact name: `POST /api/v1/cluster/join-tokens`. The server being added presents the token to `incus admin init` itself; Apelora never logs in to it.
3. **The token is shown once.** The response carries it with `Cache-Control: no-store`; it is never written to the database, the audit log, a job or a log line (`JoinToken`'s `Debug` output redacts it). The audit log records who approved which member name and when the token expires.
4. **Direct call, not a job.** Token creation, listing and revocation call Incus from the control plane synchronously, because a queued job would have to store the secret. They are quick and idempotent from the operator's point of view (issuing again replaces the old token).
5. **Short-lived.** Apelora warns when a token outlives one hour and recommends `cluster.join_token_expiry` of 1 hour or less; the test cluster uses 30 minutes. Unused tokens can be revoked (`DELETE /api/v1/cluster/join-tokens/{member}`) and are listed without their secret.
6. **Bootstrap stays on the host.** The first member is initialised on its own host (`incus admin init`, see the test-cluster scripts and, later, the runbooks); Apelora connects afterwards. Bare-metal enrolment (ADR 0002) will hand the token to a freshly installed host through its provisioning channel instead of a person.

## Consequences

- A leaked deploy certificate cannot add members, change cluster configuration or touch other projects. The admin certificate is as sensitive as Incus itself and belongs in the control plane's secret store only.
- Cluster administration bypasses the job queue, so it has no retries or idempotency keys; failures are reported to the caller directly.
- Operations that need the admin identity are easy to find: they go through `AppState.admin`.

## Alternatives

- **One unrestricted identity for everything** — simpler, but the worker (the component that runs most often and talks to Incus most) would hold full cluster control.
- **Queue token creation as a job and store the token encrypted** — adds key management for a secret that is needed exactly once, by the person who just asked for it.
- **Apelora joins servers itself over SSH** — Apelora would need credentials for every new host; the host-side join keeps that trust on the host.
