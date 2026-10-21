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
| Apelora itself | the release bundle (binaries, web apps, migrations, SBOM, licence notices, checksums, signature, documentation) carried in on approved media | planned |
| Updates | the next release bundle, verified, then installed; migrations run on start | planned (runbook) |

## What Apelora never does

- Calls home, sends telemetry or checks a licence online.
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
4. Deploy a module. If its image server has no mirror, the deployment fails with *image server … is not one of this installation's mirrors (only mirrors may be used)*. Add the mirror and retry.

Pins are kept: a mirror that serves a different image than the pinned fingerprint or digest is refused by Incus.

## Still to build

- A preflight check that every image server the catalog uses has a mirror and that the mirror answers.
- A Console page listing mirrors, their reachability, and the images each class needs.
- The signed release bundle and an offline-update runbook.
- A lab profile with outbound traffic blocked, in which the real-Incus tests must pass.
