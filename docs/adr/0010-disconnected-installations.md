# ADR 0010 — Installations without internet access (air-gapped and disconnected)

Status: **accepted; image mirrors implemented, release bundle and update path planned** · Date: 2026-10-21 · Checklist: [disconnected installations](../architecture/DISCONNECTED.md)

## Context

Many of the organisations Apelora is for run their servers without internet access:
- by policy (defence, government, critical infrastructure, regulated finance and health);
- by location (ships, remote sites, edge locations with intermittent links).

Some are fully air-gapped (no network path out at all). Others are disconnected only sometimes, or allow outbound access to a short approved list.

Apelora must install, run, update and recover in all of these, with every feature working. "Works offline" cannot be an afterthought: one hidden download (an image, a font, a key set, a licence check) breaks the whole installation.

## Decision

1. **Nothing at run time needs the internet.**
   - The control plane, the worker, the Console and the Studio make no outbound calls except to systems the operator configures: Incus, PostgreSQL, identity providers, directories, image mirrors.
   - There is no telemetry, no licence check and no call home.
   - The browser apps load nothing from outside their own origin: no web fonts, scripts or style sheets from content networks. Module logos are bundled.
2. **Images come from mirrors the installation runs.**
   - `APELORA_IMAGE_MIRRORS` maps each upstream image server (simplestreams or OCI registry) to a local mirror.
   - `APELORA_IMAGE_SERVERS=mirrors-only` refuses every server that is not a mirror, so a design cannot pull from outside even by mistake.
   - Pins (fingerprints and digests) are kept, so a mirror must serve exactly the pinned image.
   - Applied at the one place every instance is created (the adapter), in both services.
3. **Identity works inside.** Local accounts and directory sign-in (ADR 0007) need no outside service. Identity providers can be internal ones. Token verification keys are fetched from the configured provider only.
4. **Releases are offline bundles.** Each Apelora release is published as one archive:
   - the binaries and the built web apps;
   - database migrations;
   - a software bill of materials;
   - licence notices;
   - checksums and a signature;
   - the documentation.

   It is carried in on approved media, verified, then installed. Updates are the same bundle; migrations run on start, as today.
5. **Supporting content is mirrored, not fetched:**
   - operating-system packages for the servers;
   - Incus and OVN packages;
   - images;
   - vulnerability data for the scanners the operator uses.

   Apelora documents what to mirror and how to check it. It never installs packages itself.
6. **Time, names and certificates are local.**
   - NTP from inside the site.
   - DNS from inside.
   - An internal certificate authority for the API, Incus and OVN. No automatic public certificate issuance is assumed; certificate expiry is shown in the Console as today.
7. **The lab proves it.** A test profile runs the disposable cluster with outbound traffic blocked (except to the mirror). The real-Incus tests must pass there before a release is called disconnected-ready.

## Consequences

- Operators set up mirrors before the first deployment. The Console's preflight checks that every image server the catalog uses is reachable through a mirror and that mirrors-only mode is on when the installation says it is disconnected.
- Module classes keep naming their upstream image servers (so designs are portable); each installation decides where images actually come from.
- Some features depend on what the operator mirrors: a new module class needs its image in the mirror first.
- Work left:
  - the preflight check;
  - the release bundle and its signature;
  - the blocked-egress lab profile;
  - a Console page listing mirrors, their reachability and the images each class needs;
  - an offline-update runbook.

## Alternatives considered

- **A proxy that allows only approved destinations.** Useful where policy permits it, but it is still an outbound path, which many air-gapped sites forbid. Mirrors work in both cases.
- **Rewriting image servers in the catalog per installation.** Simpler, but designs exported from one installation would not run in another, and a missed class would silently reach out.
