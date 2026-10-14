---
name: xen-integration
description: Implement the Rust Node agent's integration with the upstream Xen Project hypervisor — VM lifecycle, toolstack interfaces, guest compatibility across x86-64 and ARM64. Use when touching host agent code or anything that creates, starts, stops, or inspects Xen domains.
---

# Xen integration (Node agent)

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
