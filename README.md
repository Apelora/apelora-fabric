<p align="center">
  <img src="assets/brand/logo.svg" alt="Apelora Fabric logo" width="96" height="96">
</p>

<h1 align="center">Apelora Fabric</h1>

**Distributed Application OS** — an open-source platform for composing, connecting, deploying, and operating distributed applications from reusable appliances on a cluster of Linux servers. Appliances run on [Incus](https://linuxcontainers.org/incus/) as lightweight containers (or VMs where needed); Apelora adds the application layer on top.

> Status: early-stage research and development. The orchestration system is not production-ready.

## Modules

| Module | Location | State |
|---|---|---|
| **Apelora Studio** — visual application designer (React, React Flow) | `apps/studio` | Working, browser-only |
| **Apelora Fabric Console** — cluster administration UI | `apps/fabric-console` | Placeholder |
| **Control plane** — Rust (Axum) API, graphs, plans, RBAC, audit | `services/control-plane` | Skeleton |
| **Deployment worker** — Rust reconciler, health, recovery | `services/deployment-worker` | Skeleton |
| **Incus adapter** and shared Rust domain | `crates/` | Skeleton |
| **Catalog and model** — appliance definitions, ports, schema | `packages/` | Working |

Architecture: [docs/architecture/PLATFORM-ARCHITECTURE.md](docs/architecture/PLATFORM-ARCHITECTURE.md) · decisions: [docs/adr/](docs/adr/) · plan: [docs/development/IMPLEMENTATION-PHASES.md](docs/development/IMPLEMENTATION-PHASES.md).

## Apelora Studio (browser-only preview)

The visual designer runs entirely in the browser with mock data; nothing is deployed.

```
npm install
npm run dev          # http://127.0.0.1:5173
npm run typecheck && npm run lint && npm test && npm run build
npx playwright install chromium && npm run test:e2e

# Rust workspace (needs rustup: https://rustup.rs)
cargo fmt --check && cargo clippy --workspace -- -D warnings && cargo test --workspace
```

See [docs/architecture/STUDIO-MVP.md](docs/architecture/STUDIO-MVP.md) for the design, [AGENTS.md](AGENTS.md) for project rules, and `docs/screenshots/` for current screenshots.

## Licensing

**Community:** GNU Affero General Public License version 3 only (`AGPL-3.0-only`). See [LICENSE](LICENSE) and [official license](https://www.gnu.org/licenses/agpl-3.0.html).

**Commercial:** A separate negotiated license may be available from the copyright owners. See [COMMERCIAL-LICENSE.md](COMMERCIAL-LICENSE.md). Commercial use is also permitted under AGPLv3 when its conditions are met; a commercial license is *not* mandatory merely because the user is a business.

**Marks:** Code licensing does not grant a right to use Apelora names or logos to imply official endorsement. See [TRADEMARKS.md](TRADEMARKS.md).

**Important:** Before releasing source code, replace the LICENSE placeholder with the *complete, unmodified* official AGPLv3 text (instructions below). Review upstream licences (Incus is Apache-2.0) and any contributor rights needed to support dual licensing.

## Preparing the license before publication

Run `bash scripts/install-agpl-license.sh`, inspect `LICENSE`, and commit the resulting full text. Do not publish a release until this step is complete.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). The initial contribution policy limits outside code contributions while commercial licensing rights are being designed.
