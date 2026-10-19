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
