---
name: upstream-maintenance
description: Use for tracking Incus releases, optional source reference clone, security updates and reproducible builds.
---

# Upstream Incus Version and Dependency Governance

## When to use

Use for tracking Incus releases, optional source reference clone, security updates and reproducible builds.

## Required workflow and guardrails

- Use official Incus GitHub project and linuxcontainers.org docs as sources. Check actual tags/releases, commit SHAs and package version; do not invent.
- `upstream.lock.yaml` records URL, license, exact pinned version/commit when relevant, API extensions, tested host compatibility and verification time.
- Optional source checkout under `upstream/incus` (pinned) or `.cache/upstream` (ignored); never compile it into Rust API image just because checkout exists.
- Scheduled/manual checks prepare small PRs with old/new versions, changelog, CVEs, API compatibility, integration test evidence and rollback. No unattended production updates.
- Use restricted GitHub workflow permissions; never run untrusted update PR code with infrastructure secrets.
- Lock Rust/JS deps; run license and vulnerability checks; retain third-party notices.
- Record read-only production installed release and feature extensions in Fabric integration health.

## Evidence required before calling a feature complete

Explain what changed, actual tests and commands run, source/API reference, remaining safety constraints, and whether tested against real Incus or only mocks.

## Primary references

- https://linuxcontainers.org/incus/docs/main/
- https://github.com/lxc/incus
- https://doc.rust-lang.org/cargo/reference/workspaces.html

## Apelora setup (in place since Phase 02)

- Pin: `upstream.lock.yaml` (`incus` block: `series` 7 (every 7.x release, newest first), `channel` stable, `pinned_tag`, `pinned_commit`, fork, patch branch, `tested_with`, `verified`). Change it only with `scripts/upstream/prepare-update.sh <tag>`, which resolves the tag to a commit upstream.
- Scripts (bash, no extra tools): `status.sh`, `verify.sh` (pinned tag must resolve to pinned commit), `check-updates.sh` (newer release in the series; writes `update=` to `$GITHUB_OUTPUT`), `bootstrap.sh` (read-only shallow checkout in git-ignored `upstream/incus`, remotes `origin`=upstream and `apelora`=fork), `prepare-update.sh`.
- Workflow `.github/workflows/upstream-check.yml`: weekly + manual; verifies the pin and opens an update PR on branch `upstream/incus-<tag>` (never twice for the same tag); optional `fork-sync` job keeps `Apelora/incus` main in step when the `APELORA_FORK_TOKEN` secret exists.
- Fork `Apelora/incus`: `apelora/7` = pinned tag + our patches (Apache-2.0, human DCO, no AI credit). Rebase onto each new tag; drop patches upstream accepts. Procedure: `docs/development/UPSTREAM-INCUS.md`.
- Our own features (HA, mobility, networking intents) never go into the fork unless the API cannot support them.
