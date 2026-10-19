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
