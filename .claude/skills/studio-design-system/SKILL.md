---
name: studio-design-system
description: Apply the Apelora Studio visual design contract with Tailwind CSS 4 and daisyUI 5 — white shell, framed light-gray canvas, quiet neutral styling. Use for any change to Studio layout, theming, colors, spacing, or shared UI components.
---

# Studio design system

**Inputs:** the UI change requested. **Read first:** `.claude/skills/daisyui/SKILL.md` (usage + colors) and the component guide for any daisyUI component you use.

## Contract

| Area | Rule |
|---|---|
| Shell | White (`base-100`), near-black text; slim 48px header; small neutral buttons (`btn btn-sm`); exactly one `btn-primary` per toolbar (Export) |
| Catalog (left) | 160–200px, scrolls independently, collapsible categories, 2-column compact tiles with logos |
| Canvas | Separate framed workspace: `--apl-canvas` `#F7F7F8`, 1px border, 12px radius, at most one soft frame shadow, title strip with three neutral dots, name, zoom. Dotted grid optional and faint |
| Inspector (right) | 210–270px, white, labelled fields |
| Colors | daisyUI semantic tokens only (`base-*`, `neutral`, `error`…). No `dark:` with daisyUI colors. Product logos keep vendor colors. No hard-coded grays for text |
| Themes | `light` (approved baseline, custom neutral primary), `winter`, `night`; set `--noise: 0` |
| Avoid | Gradients, glassmorphism, big status banners, heavy shadows, oversized cards |

## Procedure

1. Use daisyUI classes first, Tailwind utilities second, custom CSS last (`apps/studio/src/styles/index.css`).
2. Use the default component variant unless a variant is required.
3. Keep canvas-specific surfaces on `--apl-canvas*` variables, not on `base-*`.
4. Check light, winter and night; contrast must stay readable.

## Verification / acceptance

- `npm run typecheck && npm run lint && npx playwright test` pass (the visual-contract specs assert catalog/inspector white, canvas `rgb(247,247,248)`, 12px radius, panel widths).
- Screenshot at 1440×900 and 1280×800 reviewed (`SCREENSHOTS=1 npx playwright test tests/e2e/screenshots.spec.ts`).
