---
name: incus-platform
description: Use for cluster membership, member roles, LXC/OCI/QEMU instance and Incus REST integration.
---

# Incus API and Cluster Integration

## When to use

Use for cluster membership, member roles, LXC/OCI/QEMU instance and Incus REST integration.

## Required workflow and guardrails

- Incus is the authority for member DB, placement, images, projects and instance lifecycle. Never manipulate Incus Cowsql directly or reimplement quorum.
- First inspect documented API, check `/` and `/1.0` versions and `api_extensions`, then implement typed endpoints from verified schemas.
- Authenticate remote connections via TLS/certificates, verify server identity; no insecure SSL bypass.
- Track asynchronous operation IDs and errors. Idempotency and safe retries need explicit guardrails.
- A `minion` is an Incus cluster member in Apelora UI. Do not add a privileged host agent without ADR.
- Three members are recommended for database quorum and one voter failure; ensure synchronized clocks and sub-10ms member network latency.
- Incus's default auto placement is roughly fewest instances, not comprehensive anti-affinity. Use group/target/scriptlet capabilities only if verified.
- Ordinary LXC migrations generally need stop/move/start. No blanket live-migration claim.
- Expose unknown/unavailable resource fields instead of making them up.

## Evidence required before calling a feature complete

Explain what changed, actual tests and commands run, source/API reference, remaining safety constraints, and whether tested against real Incus or only mocks.

## Primary references

- https://linuxcontainers.org/incus/docs/main/
- https://github.com/lxc/incus
- https://doc.rust-lang.org/cargo/reference/workspaces.html

## Apelora adapter facts (verified against Incus 7.0.2 source and contract tests)

- Wire types follow `doc/rest-api.yaml` of the pinned release (read it in `upstream/incus` after `scripts/upstream/bootstrap.sh`). Lists use `?recursion=1`; instances, networks and operations also take `all-projects=true`. Envelope: `{type: sync|async|error, error, error_code, metadata}`.
- `GET /1.0/operations?recursion=1` is a list in the spec but has been grouped by status on servers; the adapter accepts both.
- A standalone (non-clustered) server has no members; check `environment.server_clustered` before calling `/1.0/cluster/members`.
- Incus generates self-signed ECDSA P-384 certificates, not CAs, with server-auth or client-auth usage. A server certificate created without host names is named only **`unspecified`**: the adapter verifies against that name automatically (or `APELORA_INCUS_TLS_SERVER_NAME`) while connecting to the configured address. Certificates must be X.509 v3; LibreSSL's `openssl req` on macOS can produce v1 certificates and EC keys rustls rejects, so generate test certificates with `rcgen` (see `crates/incus-adapter/tests/mtls.rs`).
- Configuration: `APELORA_INCUS_URL` (https only), `APELORA_INCUS_CLIENT_CERT`, `APELORA_INCUS_CLIENT_KEY`, `APELORA_INCUS_SERVER_CERT` (pinned; no public CAs trusted). No URL = labelled mock provider; URL with missing certificates = refuse to start.
