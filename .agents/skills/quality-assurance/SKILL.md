---
name: quality-assurance
description: Write and run tests across Apelora Fabric — unit, integration, browser (Playwright), and simulated-deployment tests. Use when adding features, fixing bugs, or checking whether a change is actually working.
---

# Quality assurance

## Test pyramid

| Layer | Tooling | Covers |
|---|---|---|
| Unit (TS) | Vitest | schema, validation, assembly migrations, pure UI logic |
| Component | Vitest + Testing Library | nodes, inspector, palette |
| Unit/integration (Rust) | `cargo test`, `proptest` | validation, scheduler, reconcilers, network compiler |
| Contract | shared fixtures | TS and Rust validation produce identical diagnostics |
| Browser E2E | Playwright | build a graph, connect ports, resize, import/export |
| Simulated deployment | mock Incus provider + fake storage | full reconcile from assembly to "running" without a real Incus cluster |

## Rules

1. Every bug fix starts with a failing test that reproduces it.
2. No test depends on a real Incus cluster, network access, or wall-clock time by default; real-hardware tests are opt-in behind a feature flag.
3. Fixtures live in one shared directory used by both TS and Rust.
4. Tests are deterministic: seed randomness, fake clocks.
5. For UI changes, run the app and exercise the feature in a browser; a passing type-check is not proof the feature works.

## Before reporting done

Run `npm run lint && npm test`, `cargo fmt --check && cargo clippy -- -D warnings && cargo test`, and the Playwright suite when UI changed. Report failures and skipped tests plainly.
