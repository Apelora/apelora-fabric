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
