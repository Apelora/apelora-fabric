# Apelora Fabric — project invariants for Claude Code

Authoritative rules for working in this repository. Skills in `.claude/skills/` and agents in `.claude/agents/` extend them; if a skill conflicts with this file, this file wins.

## What exists now

- **Phase 1 = browser-only Apelora Studio** (`apps/studio`): Vite + React + TypeScript + React Flow (`@xyflow/react`) + Tailwind CSS 4 + daisyUI 5. No backend, Rust, Xen, Oxia, HA or authentication yet.
- `packages/assembly-model`: zod schemas, geometry, validation, pure edit operations, (de)serialization, generated JSON Schema.
- `packages/appliance-catalog`: original appliance class manifests, bundled logos (with attribution), sample assembly.
- Product name in UI/code is **Apelora** (matching the repo, org and apelora.com). Change it in one place if branding changes (`PRODUCT_NAME`, `API_VERSION`).

## Non-negotiables

1. **Frontend only until approved.** Do not start Rust, Xen, Oxia, real HA or production auth. Never fabricate deployment results; the UI says "Mock · Design only".
2. **Domain model ≠ React Flow JSON.** Domain data (`spec`) is separate from editor layout (`editor`). Port ids (not labels/indices) are durable identity.
3. **Use React Flow viewport transforms**, never CSS `zoom`. Node size (128×96 compact) is in flow units, independent of viewport zoom.
4. **Handles:** inputs left, outputs right, stable ids, 9px marker with an 18px hit target. Call `useUpdateNodeInternals` after port/size changes.
5. **Style:** white shell, distinct light-gray framed canvas (`#F7F7F8`, 12px radius), daisyUI semantic tokens, one primary button per toolbar, no gradients/glass/heavy shadows.
6. **Imports are untrusted input.** Validate with the schema; never replace the current document on failure.
7. **No secrets in the repo.** `.mcp.json` holds non-secret config only; tokens stay in local env/keyring.
8. **Licensing:** AGPL-3.0-only code; do not edit legal agreements without an explicit request; logos need recorded origin/licence (`packages/appliance-catalog/assets/logos/ATTRIBUTION.md`); see `docs/XEN-LICENSING-STRATEGY.md`.
9. **Original work only.** Use general concepts in our own wording; never copy another product's code, artwork, text, formats, class names or manifests, and never store or quote third-party documents or mention other products by name in this repository (see `docs/architecture/COMPONENT-MODEL.md`).
10. **Git:** commits are authored by the user only. Never add `Co-Authored-By` or any Claude attribution.

## Commands

```
npm install
npm run dev            # Studio at http://127.0.0.1:5173
npm run typecheck && npm run lint && npm test && npm run build
npm run test:e2e       # Playwright (needs: npx playwright install chromium)
SCREENSHOTS=1 npx playwright test tests/e2e/screenshots.spec.ts
npm run schema         # regenerate JSON Schemas
```

## Definition of done for UI work

Typecheck, lint, unit tests, build and Playwright tests pass; behaviour verified in a real browser (not just a screenshot); failures and skipped checks reported plainly.

## MCP

Local-only, on the developer workstation. See `docs/development/MCP-HOSTING.md`. Never expose Xen management through an MCP server.
