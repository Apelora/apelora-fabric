---
name: appliance-catalog
description: Add or maintain catalog entries — categorized reusable appliances, SVG logos, and templates. Use when adding an appliance, logo, category, or starter template to the Apelora catalog.
---

# Appliance catalog

## Layout

```
catalog/
  appliances/<category>/<id>/manifest.json
  appliances/<category>/<id>/logo.svg
  templates/<id>.json        # assembly documents
```

## Rules

1. Every manifest must validate against the schema (`appliance-schema`) in CI.
2. Categories are a closed list (e.g. database, proxy, cache, storage, monitoring, runtime); adding one is a deliberate change.
3. Logos: local, sanitized SVG (no scripts, no external refs, no `foreignObject`), viewBox set, under ~20 KB. Record each logo's origin and license; third-party logos are trademarks — see `license-compliance`. Prefer a neutral generic icon over an unlicensed brand mark.
4. Image references pin a digest and list supported architectures honestly; do not claim `aarch64` without a tested image.
5. Templates must pass `topology-validation` and reference only existing appliance ids and versions.
6. Resource envelopes need a reasoned `min` and `recommended`.

## Checklist

- Manifest, logo and (if relevant) template added
- Catalog validation passes
- Entry appears in Studio palette under the right category

## Adding appliances (current process)

1. Add the class in `packages/appliance-catalog/src/classes-extra.ts` with the shorthand helpers (`i` input, `o` output, `mon()` metrics, `ver/sec/num/txt/flag` settings).
2. Follow the port placement rules in `docs/architecture/APPLIANCE-PORTS.md`: service input first, `net` last on the left; primary downstream first, `mon` last on the right; short lowercase names (≤ 8 chars); at most 3 inputs and 4 outputs.
3. Icon: a Simple Icons mark (CC0 artwork; copy the SVG, add the brand fill, record it in `assets/logos/ATTRIBUTION.md`) or a `generic-*` icon when there is no freely licensed mark.
4. Run `npm test` (catalog-quality tests check schema, icons, placement, defaults) and `npm run catalog:doc` to regenerate `docs/architecture/CATALOG.md`.
5. If a new protocol is needed, add it to `PORT_TYPES`, the layer-4 set in `compat.ts`, and a cable colour in `cables.ts`.
