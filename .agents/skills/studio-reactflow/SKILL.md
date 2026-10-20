---
name: studio-reactflow
description: Use for visual nodes, dynamic terminals, graph validation, saved topology and deployment UX.
---

# Apelora Studio React Flow Design

## When to use

Use for visual nodes, dynamic terminals, graph validation, saved topology and deployment UX.

## Required workflow and guardrails

- Preserve existing Studio code, React Flow nodes/edges/catalog, daisyUI theme and JSON import/export; inspect before rewriting.
- Provide grouped catalog, centered icons, resizable modules, dynamic left/right terminals, clear link labels, undo/redo and versioned graph save.
- Logical node = module class+instance intent; deployed replicas are separate runtime instances mapped by stable IDs.
- Build typed graph-schema shared with Rust/OpenAPI, and validate port protocols, cycles when invalid, resources, image digests, secrets refs, storage and target capabilities.
- Draw connection != working network. Explain preview of generated service routing/security plan.
- UI displays actual deployment state from backend, no mocked success or client-side Incus access.
- Good desktop density, responsive design, keyboard accessibility, and dark/light themes.
- Unit/E2E tests for drag/drop, ports, import/export roundtrip, planning, errors and deployment state.

## Evidence required before calling a feature complete

Explain what changed, actual tests and commands run, source/API reference, remaining safety constraints, and whether tested against real Incus or only mocks.

## Primary references

- https://linuxcontainers.org/incus/docs/main/
- https://github.com/lxc/incus
- https://doc.rust-lang.org/cargo/reference/workspaces.html
