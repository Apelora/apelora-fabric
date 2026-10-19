# Current state audit (Phase 01)

Date: 2026-10-20 · Repository: `github.com/Apelora/apelora-fabric`, branch `main` · Related: [ADR 0001](../adr/0001-incus-first.md), [implementation phases](../development/IMPLEMENTATION-PHASES.md)

## What exists and works

| Area | Location | State |
|---|---|---|
| Apelora Studio (designer) | `apps/studio` | Working, browser-only. React 19, TypeScript 6, Vite 8, React Flow 12, Tailwind 4 + daisyUI 5, Zustand. Catalog, canvas with typed terminals, cables and link tags, properties window, configuration forms, appliance designer, components (group/ungroup), zones and shapes, undo/redo, JSON import/export, validation panel, light/winter/night themes. Shows "Mock · Design only". |
| Domain model | `packages/assembly-model` | Zod schemas for appliance classes and application assemblies (`fabric.apelora/v1alpha1`), geometry, validation, pure edit operations, (de)serialisation, generated JSON Schema (`schema/*.json`). Domain data (`spec`) is separate from editor layout (`editor`). |
| Appliance catalog | `packages/appliance-catalog` | 83 original appliance classes with typed ports and settings, bundled logos with attribution, sample "commerce stack". |
| Tests | `packages/*/src/*.test.ts`, `tests/e2e` | 46 unit tests and 5 Rust tests pass. Playwright: 55 pass (52 Studio, 3 Fabric Console), 10 fail, 2 skipped — the 10 failures (multi-select, group into component, zones) also fail on the previous commit and are pre-existing. |
| CI | `.github/workflows/dependency-review.yml` | Dependency review only; no build/test workflow yet. |
| Agent tooling | `CLAUDE.md`, `.claude/agents`, `.agents/skills` (+ `.claude/skills` symlinks), `.mcp.json` | Skills are shared by Claude Code and Codex (Phase 01 change). MCP: Playwright and Context7, local only, no secrets. |

There is **no backend**: no Rust code, no database, no API, no Incus or other runtime integration. Nothing has been deployed anywhere.

## Earlier Xen-based assumptions

The previous plan (Rust controller + Rust node agent driving Xen, a VM per appliance) exists only in documents and agent instructions. Status after Phase 01:

| Item | Action |
|---|---|
| `docs/ARCHITECTURE.md` | Marked superseded; points to the platform architecture |
| `docs/XEN-LICENSING-STRATEGY.md` | Marked superseded (kept for history) |
| `xen-integration` skill | Marked superseded; replaced by `incus-platform` |
| Other skills and agents mentioning Xen/dom0/hypervisor | Rewritten for Incus (security-review, mcp-security, license-compliance, quality-assurance, system-design-framework, observability, fabric-networking, rust-control-plane, ui-engineer, qa-reviewer) |
| `CLAUDE.md`, `README.md` | Updated to Incus-first; project rules moved to `AGENTS.md` (read by Codex) and imported by `CLAUDE.md` |
| `SECURITY.md`, `docs/development/MCP-HOSTING.md`, `docs/architecture/STUDIO-MVP.md`, `prompts/01-build-studio.md` | Updated for Incus (prompt marked historical) |
| `COMMERCIAL-LICENSE.md`, `COMMERCIAL-AGREEMENT-DRAFT.md` | Xen references replaced with Incus/upstream platform software at the owner's explicit request (rule 8); meaning unchanged |

## What carries over unchanged

- The whole Studio app and both packages. The Incus runtime is an *addition* to an appliance class (Phase 06), not a change to existing documents.
- Port/terminal model, catalog rules, port placement convention, visual design contract.
- Licence: AGPL-3.0-only for our code. Incus (Apache-2.0) and MAAS (AGPL-3.0, if adopted per ADR 0002) are used over their APIs.

## Gaps against the target architecture

| Target | Gap | Phase |
|---|---|---|
| Cargo workspace, control-plane and worker services | Skeleton created in Phase 01 (compiles, no behaviour) | 01 → 03 |
| Upstream tracking (`upstream.lock.yaml`, update checks) | Missing | 02 |
| Incus adapter (read-only + mock), PostgreSQL persistence, auth | Missing | 03 |
| Fabric Console | Placeholder app created in Phase 01 | 05 |
| Shared API contract (OpenAPI → TS client) | Missing | 03 |
| Network fabric, service discovery, publishing | Missing | 07 |
| Coordination, HA, mobility | Missing | 08 |
| Bare-metal provisioning and enrolment | Missing; ADR 0002 proposed | 09 |
| Build/test CI | Missing | 10 |

## Deviations from the incoming specification (deliberate)

- **npm workspaces kept** instead of pnpm (owner decision): no tooling churn; Fabric Console is another workspace.
- **Crates created when first needed**, not all at once: Phase 01 adds `crates/domain` and `crates/incus-adapter` plus the two services; the other planned crates are listed in the architecture but created by the phase that gives them code.
- **No other product names** in repository text (rule 9); the incoming documents were rewritten in our own words and the `files/` drop folder removed.
- **No gradients** in UI themes (design rule 5), although the incoming spec mentions gradient themes.
- Repository stays `Apelora/apelora-fabric` (the spec says `Apelora/apelora`).
