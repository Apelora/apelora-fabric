# Running Apelora without internet access

Status: **checklist** · Date: 2026-10-21 · Decision: [ADR 0010](../adr/0010-disconnected-installations.md)

What an air-gapped or disconnected installation needs, what Apelora already does, and what the operator provides.

## What Apelora reaches, and what you provide inside

| Apelora needs | Inside a disconnected site | Status |
|---|---|---|
| Incus on every server | Incus packages from your package mirror | provided by you |
| OVN (application networks) | OVN packages from your package mirror | provided by you |
| PostgreSQL | a PostgreSQL inside the site | provided by you |
| Module images (system containers, VMs, OCI images) | an image mirror (a simplestreams server and an OCI registry) holding the images your module classes pin; `APELORA_IMAGE_MIRRORS` and `APELORA_IMAGE_SERVERS=mirrors-only` | **built** (mirrors applied in both services) |
| Sign-in | local accounts or a directory inside (ADR 0007), or an internal identity provider | identity providers built; local accounts and directory in progress |
| Time | NTP servers inside the site | provided by you |
| Names | DNS inside the site | provided by you |
| Certificates | an internal certificate authority for the API, Incus and OVN | provided by you; expiry shown in the Console |
| Apelora itself | the release bundle (binaries, web apps, migrations, software inventory, licence notices, checksums, signature, documentation) carried in on approved media; `scripts/release/bundle.sh`, checked offline with `scripts/release/verify.sh` | **built** (built and verified locally; no release published yet) |
| Updates | the next release bundle, verified, then installed; migrations run on start | **runbook** ([offline-update.md](../runbooks/offline-update.md), not yet carried out on an installation) |
| Monitoring | a metrics collector, its storage and its alerting inside the site; it scrapes Apelora's `/metrics`, every Incus server's `/1.0/metrics` with a metrics-only certificate, PostgreSQL and the members ([monitoring](MONITORING.md)) | **built** (Apelora's endpoints); the collector is provided by you |

## What Apelora never does

- Calls home, sends telemetry or checks a licence online. Metrics are only answered when the collector inside the site asks; nothing is pushed.
- Loads fonts, scripts or styles from outside its own origin in the browser.
- Installs packages on servers.
- Pulls an image from a server that is not one of the installation's mirrors (with `APELORA_IMAGE_SERVERS=mirrors-only`).

## Setting up image mirrors

1. List the images your module classes pin: the Studio's catalog shows each class's image server, alias and fingerprint or digest.
2. Copy those images into your mirror on a connected system, carry them in, and import them:
   - simplestreams images go to an image server inside;
   - OCI images go to a registry inside.
3. Configure both services:

   ```sh
   APELORA_IMAGE_MIRRORS="https://images.upstream.example=https://images.mirror.internal,https://registry.upstream.example=https://registry.mirror.internal"
   APELORA_IMAGE_SERVERS=mirrors-only
   ```

   At start, each service logs `images come from this installation's mirrors` with the number of mirrors and the mode.
4. Check: **Preflight** and **Administration → Image mirrors** in the Fabric Console (`GET /api/v1/image-mirrors`, needs `cluster.view`) show:
   - the mode, and a warning when mirrors are set but other servers are still allowed, or when none are set (fine with internet access);
   - each upstream → mirror pair and whether the mirror answers, with the time it was checked: an HTTPS GET of `/streams/v1/index.json` (simplestreams) or `/v2/` (OCI registry; 200 or 401 both count as answering), 3 s timeout, redirects not followed, certificates checked against the system's trust store (install your internal certificate authority there);
   - each image server the catalog's deployable classes use, the classes and settings that need it, and whether it is mirrored, pulled directly, or refused.

   Each mirror is probed at most once a minute, however often the page or Preflight is opened, and upstream servers are never contacted. The view shows what the control plane read at start; the worker reads its own environment, so set both the same.
5. Deploy a module. If its image server has no mirror, the deployment fails with *image server … is not one of this installation's mirrors (only mirrors may be used)*. Add the mirror and retry.

Pins are kept: a mirror that serves a different image than the pinned fingerprint or digest is refused by Incus.

Evidence: the mirror rules and the checks are CONTRACT VERIFIED (unit and API tests, probes against local test servers); the Console page is MOCK VERIFIED (Playwright with a stubbed API). Nothing here has run against a real mirror yet.

## Releases and updates

- `scripts/release/bundle.sh` builds `dist/apelora-<version>.tar.gz` on a connected system: the two services, the two web apps, the migrations, the documentation, licence notices, a software inventory (`inventory/cargo.json`, `inventory/npm.json`), `BUILD-INFO`, `SHA256SUMS`, and `SHA256SUMS.sig`, an OpenSSH signature (`ssh-keygen -Y sign`, namespace `apelora-release`) made with the key in `APELORA_RELEASE_SIGNING_KEY`. It refuses to sign without a key and refuses a key inside the repository; it never creates one.
- `scripts/release/verify.sh <bundle> <allowed_signers>` checks the signature against the release keys you trust, every checksum, and that the bundle holds nothing unlisted; it needs only OpenSSH, `tar` and `sha256sum`/`shasum`.
- Install and update: [offline-update.md](../runbooks/offline-update.md).

## Still to do

- Run the real-Incus tests in the lab with outbound traffic blocked (`scripts/test-cluster/egress.sh`, [TEST-CLUSTER.md](../development/TEST-CLUSTER.md)) and a mirror inside; the profile is built and its rules are checked, but it has not been run.
- Carry out the offline update on an installation and record the result in the runbook.
