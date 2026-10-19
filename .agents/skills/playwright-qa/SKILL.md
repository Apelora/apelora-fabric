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

## Required coverage (see `docs/architecture/STUDIO-MVP.md`)

Visual contract; compact node geometry; six-node fit; catalog collapse/search/add; drop at multiple zoom levels and after pan; move/resize/density with anchored edges; 3–8 ports; connection validation; property edits, delete, undo/redo, import/export, persistence; keyboard and accessible names; console/network hygiene.
