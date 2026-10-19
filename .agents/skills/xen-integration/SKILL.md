---
name: xen-integration
description: SUPERSEDED by incus-platform (ADR 0001, Incus-first). Historical notes on the earlier Xen node-agent design; do not use for new work.
---

# Xen integration (Node agent) — SUPERSEDED

> **Superseded by ADR `docs/adr/0001-incus-first.md`.** Apelora no longer drives a hypervisor directly; instances run on Incus (LXC, OCI, optional QEMU VMs) through its REST API. Use `incus-platform`. This file is kept for history only.

Target: upstream Xen directly. **No XCP-ng/XAPI dependency.**

## Design

- Hide Xen behind a `Hypervisor` trait (`create`, `start`, `stop`, `destroy`, `status`, `attach_disk`, `attach_nic`, `list`). All other code depends on the trait; ship a fake implementation for tests and CI.
- The real backend drives the Xen toolstack (`xl`/libxl domain config). Prefer a library binding; if shelling out, build argv arrays (never string-concatenate), use a generated config file with strict permissions, and parse structured output.
- Domain lifecycle is a state machine; operations are idempotent and keyed by an Apelora-owned domain name/UUID so the agent can rediscover and adopt domains after restart.
- Guest types: PV/PVH/HVM per image metadata; the manifest architecture must match the host. Never boot a mismatched-arch image.
- Resource limits (vCPU, memory, disk, network) come from the validated desired state, not raw user input.

## Hard constraints

- Agent runs with the least privilege that works, in dom0; treat all controller input as untrusted and validate paths, names, and sizes.
- Toolstack and libxl licensing (GPL/LGPL) affects how the Node is linked and distributed — consult `license-compliance` before adding any Xen-derived code or linking.
- Hardware-dependent tests are gated behind a feature flag; default CI uses the fake backend.

## Checklist

- Trait + fake updated together
- Lifecycle edge cases: already-running, missing domain, crash mid-create
- No shell injection surface; no secrets in logs or domain configs
