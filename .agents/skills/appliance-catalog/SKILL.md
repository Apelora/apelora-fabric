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

## Runtimes (Incus-first, ADR 0001)

Every appliance class declares how it runs on Incus. A class without a runtime is a design-only class (Studio still works; it cannot be deployed).

| Runtime | Use for | Notes |
|---|---|---|
| `incus-system-container` (LXC) | Default: web servers, APIs, workers, caches, databases | Full init system; unprivileged by default |
| `incus-oci-container` | Single-process apps shipped as OCI images | Pin the image by digest; verify entrypoint and behaviour at runtime |
| `incus-vm` (QEMU) | Firewalls/routers, other kernels or OSes (Windows), strong isolation | Exception, not the default |
| `composite` | A Studio grouping of other appliances | Not an Incus instance type |

7. Deployable classes record: versioned id, approved image with immutable digest, architectures, licence/provenance, CPU/RAM/disk limits, terminals, configuration schema, **secret references** (never real secrets), volumes, health probes, rollout and placement policy, availability class (stateless / singleton / stateful).
8. Default to unprivileged containers in restricted projects; refuse broad host device or path mounts for ordinary appliances. If an appliance needs privileges or kernel features a container cannot safely have (e.g. a general-purpose firewall), make it a VM.
9. Reject unsupported or mismatched terminal protocols and unresolved dependencies before deployment.
10. Keep reference fixtures for an nginx web tier, an API, a worker, PostgreSQL (stateful safety rules) and one optional VM.

## Checklist

- Manifest, logo and (if relevant) template added
- Catalog validation passes
- Entry appears in Studio palette under the right category

## Adding appliances (current process)

1. Add the class in `packages/appliance-catalog/src/classes-extra.ts` with the shorthand helpers (`i` input, `o` output, `mon()` metrics, `ver/sec/num/txt/flag` settings).
2. Follow the port placement rules in `docs/architecture/APPLIANCE-PORTS.md`: service input first, `net` last on the left; primary downstream first, `mon` last on the right; short lowercase names (≤ 8 chars); at most 3 inputs and 4 outputs.
3. Icon: a Simple Icons mark (CC0 artwork; copy the SVG, add the brand fill, record it in `assets/logos/ATTRIBUTION.md`) or a `generic-*` icon when there is no freely licensed mark.
4. Run `npm test` (catalog-quality tests check schema, icons, placement, defaults) and `npm run catalog:doc` to regenerate `docs/architecture/CATALOG.md`.
5. If a new protocol is needed, add it to `PORT_TYPES`, the layer-4 set in `compat.ts`, and a protocol cable colour in `apps/studio/src/components/canvas/cables.ts` (users may still override individual cables).
