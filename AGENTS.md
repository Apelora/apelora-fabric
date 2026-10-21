# Apelora Fabric — project rules for coding agents

Authoritative rules for working in this repository, shared by all coding agents (Codex reads this file; Claude Code imports it from `CLAUDE.md`). Skills live in `.agents/skills/` (Claude sees them through symlinks in `.claude/skills/`). If a skill conflicts with this file, this file wins.

## Architecture in one paragraph

Apelora composes applications from modules in a visual designer and runs them on a cluster of Linux servers. **Incus** runs the modules (LXC system containers by default, OCI application containers, QEMU VMs where needed) and owns cluster membership, placement and instance lifecycle. **Apelora** owns application graphs, module classes, desired state, connectivity, replicas, health and recovery, in a Rust control plane backed by PostgreSQL. See [ADR 0001](docs/adr/0001-incus-first.md), [ADR 0003](docs/adr/0003-incus-identities-and-cluster-join.md) (Incus identities, cluster join), [ADR 0004](docs/adr/0004-service-discovery.md) (service discovery), [ADR 0006](docs/adr/0006-modules.md) (modules), [platform architecture](docs/architecture/PLATFORM-ARCHITECTURE.md), [implementation phases](docs/development/IMPLEMENTATION-PHASES.md) and the [current state](docs/architecture/current-state.md).

## What exists now

- `apps/studio` — the designer: Vite + React + TypeScript + React Flow (`@xyflow/react`) + Tailwind CSS 4 + daisyUI 5; deploys designs through the API and overlays live state.
- `apps/fabric-console` — administrator console: inventory, instances, operations, preflight, membership (machine enrolment, evacuation, join tokens), audit, tokens.
- `packages/api-client` — TypeScript types and client generated from the control plane's OpenAPI document.
- `packages/assembly-model` — zod schemas, geometry, validation, pure edit operations, network intent compiler, (de)serialisation, generated JSON Schema.
- `packages/module-catalog` — original module class manifests, runtimes (NGINX, Debian), bundled logos (with attribution), sample assembly.
- `services/control-plane` (Axum API), `services/deployment-worker` (job queue, network sync, replica supervisor), `crates/domain`, `crates/persistence` (PostgreSQL), `crates/incus-adapter` (Incus REST + mock). **REAL INCUS TESTED** on a disposable four-member cluster (Lima VMs standing in for physical servers); see `docs/architecture/current-state.md` for what is verified and the known limitations.
- Product name in UI/code is **Apelora**. Change it in one place if branding changes (`PRODUCT_NAME`, `API_VERSION`).

## Non-negotiables

1. **Incus-first, one phase at a time.** Work follows the implementation phases in order. Incus is reached only through its documented REST API with TLS verification; never write its database, fork it or build a competing scheduler/cluster manager. No infrastructure mutation except on an explicitly designated, disposable test cluster after a dry run. Never fabricate deployment, telemetry or HA results; label results MOCK VERIFIED / CONTRACT VERIFIED / REAL INCUS TESTED. The UI says "Mock · …" until it shows real backend data.
2. **Domain model ≠ React Flow JSON.** Domain data (`spec`) is separate from editor layout (`editor`). Port ids (not labels/indices) are durable identity.
3. **Use React Flow viewport transforms**, never CSS `zoom`. Node size (128×96 compact) is in flow units, independent of viewport zoom.
4. **Handles:** inputs left, outputs right, stable ids, 9px marker with an 18px hit target. Call `useUpdateNodeInternals` after port/size changes.
5. **Style:** white shell, distinct light-gray framed canvas (`#F7F7F8`, 12px radius), daisyUI semantic tokens, one primary button per toolbar, no gradients/glass/heavy shadows. Fabric Console uses the same theme.
6. **Imports are untrusted input.** Validate with the schema; never replace the current document on failure.
7. **No secrets in the repo.** `.mcp.json` holds non-secret config only; tokens, Incus client certificates and BMC credentials stay in local env/keyring or the backend's secret store — never in the browser, logs or the repository. The browser talks only to the Apelora API.
8. **Licensing:** AGPL-3.0-only code; do not edit legal agreements without an explicit request; logos need recorded origin/licence (`packages/module-catalog/assets/logos/ATTRIBUTION.md`). Upstreams are used over their APIs (Incus Apache-2.0; MAAS AGPL-3.0 if adopted); keep their notices.
9. **Original work only.** Use general concepts in our own wording; never copy another product's code, artwork, text, formats, class names or manifests, and never store or quote third-party documents or mention other products by name in this repository (dependencies we actually use, such as Incus, PostgreSQL or OVN, may be named). See `docs/architecture/COMPONENT-MODEL.md`.
10. **Git:** commits are authored by the user only. Never add `Co-Authored-By` or any AI attribution.

## Commands

```sh
npm install
npm run dev              # Studio at http://127.0.0.1:5173
npm run dev:console      # Fabric Console at http://127.0.0.1:5174 (proxies /api to 127.0.0.1:8080)
DATABASE_URL= cargo run -p apelora-control-plane   # API at http://127.0.0.1:8080, mock provider, no sign-in (development only)
cargo run -p apelora-control-plane   # same, with the dev database from .cargo/config.toml: sign-in required
docker compose -f infra/dev/compose.yaml up -d   # PostgreSQL 18 on 127.0.0.1:15432 for Rust tests and the worker
DATABASE_URL=… cargo run -p apelora-control-plane -- token create --name me --role fabric-admin   # API token (shown once)
npm run api:generate   # after API changes: OpenAPI document + TypeScript client (packages/api-client)
npm run typecheck && npm run lint && npm test && npm run build
npm run test:e2e         # Playwright (needs: npx playwright install chromium)
cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
SCREENSHOTS=1 npx playwright test tests/e2e/screenshots.spec.ts
npm run schema           # regenerate JSON Schemas
cargo deny check && node scripts/licenses/check-npm.mjs   # dependency licences, advisories, sources
bash scripts/security/scan-secrets.sh   # no private keys or tokens in tracked files
bash scripts/upstream/status.sh   # pinned Incus vs upstream (see docs/development/UPSTREAM-INCUS.md)
bash scripts/test-cluster/up.sh && source ~/.config/apelora/test-cluster/env   # disposable Incus in a Lima VM (docs/development/TEST-CLUSTER.md)
DATABASE_URL=… cargo test -p apelora-deployment-worker --test real_incus -- --ignored --test-threads 1   # REAL INCUS TESTED lifecycle
```

## How to change things

- Before starting, read the sections of [lessons learned](docs/development/LESSONS-LEARNED.md) that touch your area; add an entry when something fails for a non-obvious reason or the owner corrects how we work.
- Start from the nearest existing implementation and its tests; extend existing helpers instead of adding parallel ones.
- Keep changes focused; prefer the smallest change that fits the current design.
- New dependencies (npm or Cargo), new public Rust items and new crates are architectural decisions: justify them, check their licence (`license-compliance`), and prefer what is already in the tree.
- For a bug, first reproduce it with the smallest failing test, then fix it.
- Generated files (`docs/architecture/CATALOG.md`, `packages/assembly-model/schema/*.json`, `packages/api-client/openapi.json`, `packages/api-client/src/generated.ts`) are regenerated with their scripts, never edited by hand.

## Definition of done

- **UI work:** typecheck, lint, unit tests, build and Playwright tests pass; behaviour verified in a real browser (not just a screenshot).
- **Rust work:** fmt, clippy (`-D warnings`) and `cargo test --workspace` pass; behaviour against Incus labelled by evidence level.
- Failures and skipped checks reported plainly. Known pre-existing failures are listed in `docs/architecture/current-state.md`.

## Which skill for which task

| Task | Skills |
|---|---|
| Fabric Console views and actions | `fabric-console`, `studio-design-system`, `daisyui`, `playwright-qa` |
| Studio canvas, nodes, interactions | `visual-designer`, `compact-module`, `react-flow-interactions`, `studio-reactflow`, `studio-design-system`, `daisyui` |
| Catalog and schema | `module-catalog`, `module-schema`, `application-assembly`, `topology-validation` |
| Rust API, worker, jobs | `rust-control-plane`, `security-tenancy`, `observability` |
| Incus integration | `incus-platform`, `upstream-maintenance` |
| Upstream knowledge (licences, facts, adopted practices, upstream AI policies) | `upstream-reference` — add an entry whenever an upstream repo or document is shared |
| Networking | `fabric-networking`, `incus-networking` |
| HA, storage, recovery | `reconciliation-ha`, `storage-recovery`, `storage-volumes` |
| Tests and evidence | `testing-operations`, `quality-assurance`, `playwright-qa` |
| Security, licensing, agent tools | `security-review`, `identity-authorization`, `license-compliance`, `mcp-security` |
| Identity, roles, scopes, approvals, audit | `identity-authorization`, `security-tenancy`, `rust-control-plane` |
| Application management (drafts, versions, Start/Stop/Restart) | `application-lifecycle`, `studio-design-system`, `playwright-qa` |
| Terminal and PowerShell into modules (built; RDP not built) | `secure-remote-access`, `identity-authorization`, `security-review` |
| Mutation testing, strengthening tests | `mutation-testing`, `testing-operations`, `quality-assurance` |
| Design work and scaling | `system-design-framework`, `capacity-estimation`, `scalability-patterns` |

## MCP

Local-only, on the developer workstation. See `docs/development/MCP-HOSTING.md`. Never expose Incus cluster management through an MCP server.
