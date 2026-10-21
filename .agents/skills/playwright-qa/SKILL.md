---
name: playwright-qa
description: Test Apelora Studio in a real browser with Playwright (test runner and MCP) — run the suite, add specs for new interactions, capture screenshots, and report results truthfully. Use whenever UI behavior changes or before claiming a UI feature works.
---

# Playwright QA

## Setup

```
npx playwright install chromium     # once
npm run dev                          # 127.0.0.1:5173 (the config also starts it if needed)
npm run test:e2e
SCREENSHOTS=1 npx playwright test tests/e2e/screenshots.spec.ts   # docs/screenshots/*
```
Playwright MCP is configured for Chromium in `.mcp.json` (restart Claude Code after changes). If it cannot launch, fall back to the test runner and say so.

## Rules

1. A spec asserts behavior, not appearance alone: check store state through `window.__studio` (dev builds only), DOM geometry, and edge anchoring (`expectEdgesAnchored`).
2. Each new interaction gets a spec in `tests/e2e/studio.spec.ts` using helpers in `tests/e2e/helpers.ts`; tests start from a clean `localStorage` via `open(page)`.
3. Assert: no console errors, no requests to non-localhost hosts.
4. Wait on conditions (`expect.poll`, locators), not fixed sleeps, except for short animation settling.
5. Never report a test as passed unless it ran; report skipped/blocked checks and why.
6. **Cross-platform modifiers.** For additive selection use `click({ modifiers: ["ControlOrMeta"] })` and `keyboard.press("ControlOrMeta+…")`. A plain `Control`-click is a right-click on macOS, so such tests pass on Linux CI and fail on Mac.
7. **Prove a regression test.** Before keeping an app fix, show the new test fails without it (e.g. `git stash` the fix, rerun). If the test passes without the fix, the fix is unproven: revert it and keep the test as coverage only.
8. Fabric Console specs live in `tests/e2e/fabric-console.spec.ts` (project `console-1440`, base URL `:5174`); mock the API with `page.route("**/api/v1/…")` so tests never depend on a running backend.

## CI (`docs/development/CI.md`, skill `ci-pipeline`)

Every push and pull request runs only the **`@smoke`** tests (`ci.yml`, about 35 s); the whole suite runs nightly, by hand and on pull requests labelled `full-e2e` (`e2e-full.yml`, four shards split by test). Tag one or two tests per new area with `{ tag: "@smoke" }` (details argument, not the title), and run the whole suite locally (`npm run test:e2e`) before pushing UI work. Tests must not depend on another test in the same file having run first: shards split files. `forbidOnly` is on in CI. No secrets or Incus credentials exist in CI; never add live-infrastructure tests there.

## Required coverage (see `docs/architecture/STUDIO-MVP.md`)

Visual contract; compact node geometry; six-node fit; catalog collapse/search/add; drop at multiple zoom levels and after pan; move/resize/density with anchored edges; 3–8 ports; connection validation; property edits, delete, undo/redo, import/export, persistence; keyboard and accessible names; console/network hygiene.
