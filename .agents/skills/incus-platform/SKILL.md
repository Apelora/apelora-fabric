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
- In Apelora a server is an Incus cluster member, called a server (or member) in the UI; in production it is a physical server, in the lab a Lima VM stands in for one. Do not add a privileged host agent without ADR.
- Three members are recommended for database quorum and one voter failure; ensure synchronized clocks and sub-10ms member network latency. The lab has four (three voters, one standby) plus a SAN VM (`docs/development/TEST-CLUSTER.md`).
- Evacuation (verified on 7.0.1, two members): `POST /1.0/cluster/members/<name>/state {"action":"evacuate"|"restore"}` with the admin identity; containers on local btrfs with OVN NICs moved (cold) in seconds and kept their addresses; restore moved them back. `cluster.evacuate` comes from the expanded config (profiles included).
- Incus's default auto placement is roughly fewest instances, not comprehensive anti-affinity. Apelora's replica anti-affinity is the placement scriptlet `infra/incus/placement.star` (REAL INCUS TESTED on two members); install with `KEY=VALUE` (`incus config set "instances.placement.scriptlet=$(cat …)"`), call scriptlet helpers with keyword arguments, and leave unlabelled instances to Incus.
- **An offline member's instances (read in the 7.0.2 source, REAL INCUS TESTED by `ha-drill.sh`):** `POST /1.0/instances/<name>?target=<member>` with `{"migration": true}` relinks an instance on a remote pool (`lvmcluster`, `ceph`) away from an offline member (records only, it is then stopped there); a member-local pool fails; `target=@group` is refused while the source is offline (the request is forwarded to the target, which must be a member). `cluster.healing_threshold` (default 0) does the same for every remote-pool instance of every project on the leader once a minute after one failed ping, and marks the member Evacuated; Apelora keeps it off (ADR 0013). Evacuating an offline member through the API is forwarded to it and fails.
- Ordinary LXC migrations generally need stop/move/start. No blanket live-migration claim: only VMs with `migration.stateful=true` and no attached member-local volumes move running (`instance.move`, REAL INCUS TESTED).
- Expose unknown/unavailable resource fields instead of making them up.

## Evidence required before calling a feature complete

Explain what changed, actual tests and commands run, source/API reference, remaining safety constraints, and whether tested against real Incus or only mocks.

## Primary references

- https://linuxcontainers.org/incus/docs/main/
- https://github.com/lxc/incus
- https://doc.rust-lang.org/cargo/reference/workspaces.html

## Apelora adapter facts (verified against Incus 7.0.2 source and contract tests)

- **Incus 7.5.1 (REAL INCUS TESTED 2026-10-30, ADR 0017):** the pin follows the newest release; the adapter and the worker needed no code change from 7.0.2. Run a release through `scripts/test-cluster/upgrade-incus.sh`, the `real_incus` tests, `demo.sh` and `ui-e2e.sh` before moving the pin. The cluster database upgrades one way. The `stable` package channel holds only its newest build.
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
- **Holding a member** (Phase 09 enrolment): member config `scheduler.instance=manual` keeps a member out of automatic placement while targeted instances still land on it; Incus cannot preset it before the join, so the enrolment script (`scripts/enrol/enrol-member.sh`) sets it right after. Update a member with `PATCH /1.0/cluster/members/<name>` carrying its `roles`, `groups`, `failure_domain`, `description` *and* the config — a missing `roles` is read as dropping them (`The "database" role cannot be dropped`); an empty value removes a key. `GET /1.0?target=<member>` gives that member's own versions. Unmanaged interfaces list `"locations": null` (decode as optional); OVN networks list only the members present when they were created, so never treat a missing OVN location as a defect.
- **Join tokens** (`POST /1.0/cluster/members {server_name}`) need an unrestricted identity. The async response's operation (`class: token`) carries `metadata {serverName, secret, fingerprint, addresses, expiresAt}`; the token a new member gives `incus admin init` is base64 of JSON `{server_name, fingerprint, addresses, secret, expires_at}`. Issuing again for the same name cancels the older token. Pending tokens are running `token` operations in `GET /1.0/operations?recursion=1`; revoke with `DELETE /1.0/operations/{id}`. Expiry: `cluster.join_token_expiry` (default `3H`; units `S M H d w m y`, `M` = minutes). The joining preseed needs `cluster_token`, `server_address` and member-specific storage config. A sized loop pool named `local` needs both `size` and an empty `source` (Incus then creates the loop file). `scripts/enrol/enrol-member.sh --pool-size 20GiB` writes those two keys; a disk uses `--member-config`. The switch itself has not been run against a cluster.
- **Projects for environments** (REAL INCUS TESTED, Incus 7.0.x, four members, 2026-10-21; `crates/incus-adapter/src/tenancy.rs`, ADR 0012): `POST /1.0/projects {name, description, config}`, `PUT /1.0/projects/<name> {description, config}` (replaces the config: send it whole), `PUT /1.0/profiles/default?project=<name>`, `DELETE /1.0/projects/<name>` are synchronous and need the admin identity; a restricted certificate gets `403 Certificate is restricted`. `GET /1.0/projects/<name>/state` answers `resources.<instances|cpu|memory|disk|…>` with **capitalised `Limit`/`Usage`** (the Go struct has no JSON tags; `Limit` -1 = none; memory and disk in bytes, cpu = sum of instances' `limits.cpu`). A limit refusal comes back as **HTTP 500**, not 403: "Reached maximum number of instances in project", "Reached maximum aggregate value \"2\" for \"limits.cpu\"", and it happens before any image is fetched. Adding a project to a restricted certificate: `PATCH /1.0/certificates/<fp> {"projects": [...]}` (merged into the entry); the deploy identity could read the project immediately afterwards. Deleting a project removes it from every restricted certificate's list. With `limits.disk`, every instance needs a root disk `size` (directly or via a profile) and every custom volume a `size`; with `limits.cpu`/`limits.memory`, every instance needs that key.
- **Clustering reads** (REAL INCUS TESTED read-only, Incus 7.0.1, four members, 2026-10-21; used by `GET /api/v1/cluster/members|groups|settings`): member `roles` hold `database` (voter), `database-leader` (together with `database`), `database-standby`; `database: true` is set for stand-bys too, so read the roles, not that flag. `event-hub`, `ovn-chassis` and `database-client` appear only when assigned (the lab's OVN members do not list `ovn-chassis`). Member statuses: Online, Evacuating, Evacuated, Restoring, Offline, Blocked (an older member while a newer release waits). The restricted deploy identity reads members, `/1.0/cluster/groups?recursion=1` (`config: null`, `members` may be `null`), `/1.0?target=<member>` (that member's release) and `/1.0/metadata/configuration` (`configs.server.<section>.keys[]` with `defaultdesc` such as `` `20` ``); `GET /1.0` gives it an **empty `config`**, so cluster settings are read with the admin identity, and only allow-listed keys leave the adapter (`server_config(keys)`): the configuration holds secrets and the placement scriptlet.
