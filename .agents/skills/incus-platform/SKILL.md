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
- Evacuation (verified on 7.0.1, two members): `POST /1.0/cluster/members/<name>/state {"action":"evacuate"|"restore"}` with the admin identity; containers on local btrfs with OVN NICs moved (cold) in seconds and kept their addresses; restore moved them back. `cluster.evacuate` comes from the expanded config (profiles included).
- Incus's default auto placement is roughly fewest instances, not comprehensive anti-affinity. Apelora's replica anti-affinity is the placement scriptlet `infra/incus/placement.star` (REAL INCUS TESTED on two members); install with `KEY=VALUE` (`incus config set "instances.placement.scriptlet=$(cat …)"`), call scriptlet helpers with keyword arguments, and leave unlabelled instances to Incus.
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
- Clustered servers present the **cluster certificate**, named after the member's host name (plus loopback), not the address clients dial: set `APELORA_INCUS_TLS_SERVER_NAME` to that name (`scripts/test-cluster/up.sh` reads it from the certificate). The adapter refuses to guess.

## Facts from the disposable test cluster (REAL INCUS TESTED, Incus 7.0.1, Debian 13 arm64, 2026-10-20)

Runbook: `docs/development/TEST-CLUSTER.md` (`scripts/test-cluster/up.sh`, `down.sh`). Real tests: `services/deployment-worker/tests/real_incus.rs` (ignored by default; refuse to run against the mock).

- **`"profiles": []` applies no profiles** (no root disk: "No root device could be found"); only an absent list gets `default`. Send the field only when non-empty.
- **Instance source fingerprint wins over the alias** (`source.fingerprint`), so a pinned simplestreams image is exactly that image. For OCI the fingerprint would be passed to the registry client as a reference: pin OCI images with a digest in the alias (`repository@sha256:…`; a multi-arch index digest works).
- **`volatile.base_image`** records the image fingerprint of every instance, OCI included; for OCI it is Incus's own hash, not the registry's manifest digest. Compare pins against it.
- Incus places an instance only on members whose architecture the image offers (`SuitableArchitectures`); `InstancesPost.architecture` is ignored for image sources. So Apelora checks before creating (`lifecycle::placeable`): a standalone server against `environment.architectures`, a cluster against its members, and in a cluster that mixes architectures only a fingerprint pin is accepted. Never let a create succeed and then refuse the result.
- **A VM still booting may ignore a graceful stop** ("Failed shutting down instance … context deadline exceeded" after the 30 s timeout). The job retries with backoff and succeeds; do not escalate to force on your own.
- A client certificate restricted to one project can still read cluster members, managed networks and storage pools, and sees only its own projects: inventory and preflight work with least privilege.
- With `instance_create_start`, `POST /1.0/instances` with `start: true` returns one operation that covers create and start.
- OCI support (`instance_oci`) works out of the box with the 7.0 LTS packages; a cached image makes a second create take seconds.
- **Join tokens** (`POST /1.0/cluster/members {server_name}`) need an unrestricted identity. The async response's operation (`class: token`) carries `metadata {serverName, secret, fingerprint, addresses, expiresAt}`; the token a new member gives `incus admin init` is base64 of JSON `{server_name, fingerprint, addresses, secret, expires_at}`. Issuing again for the same name cancels the older token. Pending tokens are running `token` operations in `GET /1.0/operations?recursion=1`; revoke with `DELETE /1.0/operations/{id}`. Expiry: `cluster.join_token_expiry` (default `3H`; units `S M H d w m y`, `M` = minutes). The joining preseed needs `cluster_token`, `server_address` and member-specific storage config (e.g. a loop pool's `size`).
