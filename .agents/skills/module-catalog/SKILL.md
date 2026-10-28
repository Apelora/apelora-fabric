---
name: module-catalog
description: Add or maintain catalog entries — categorized reusable modules, SVG logos, and templates. Use when adding a module, logo, category, or starter template to the Apelora catalog.
---

# Module catalog

## Layout

```
catalog/
  modules/<category>/<id>/manifest.json
  modules/<category>/<id>/logo.svg
  templates/<id>.json        # assembly documents
```

## Rules

1. Every manifest must validate against the schema (`module-schema`) in CI.
2. Categories are a closed list (e.g. database, proxy, cache, storage, monitoring, runtime); adding one is a deliberate change.
3. Logos: local, sanitized SVG (no scripts, no external refs, no `foreignObject`), viewBox set, under ~20 KB. Record each logo's origin and license; third-party logos are trademarks — see `license-compliance`. Prefer a neutral generic icon over an unlicensed brand mark.
4. Image references pin a digest and list supported architectures honestly; do not claim `aarch64` without a tested image.
5. Templates must pass `topology-validation` and reference only existing module ids and versions.
6. Resource envelopes need a reasoned `min` and `recommended`.

## Runtimes (Incus-first, ADR 0001)

Every module class declares how it runs on Incus. A class without a runtime is a design-only class (Studio still works; it cannot be deployed).

| Runtime | Use for | Notes |
|---|---|---|
| `incus-system-container` (LXC) | Default: web servers, APIs, workers, caches, databases | Full init system; unprivileged by default |
| `incus-oci-container` | Single-process apps shipped as OCI images | Pin the image by digest; verify entrypoint and behaviour at runtime |
| `incus-vm` (QEMU) | Only where needed: another kernel or OS (Windows, `guestOs`), kernel modules, firewall-style modules, or live migration (`migration.stateful`) | Exception, not the default. Today: `linux-base`, a VM so it can move while running |

Today's runnable classes are listed in `packages/module-catalog/src/runtimes.ts` (OCI containers such as `nginx` and `valkey`, system containers such as `debian`, and the `linux-base` VM). A component (`composition` on the class) is not a runtime: it is a Studio grouping, deployed as its parts in a later version. The whole chain from module to instances, members, pools and networks: `docs/architecture/PLATFORM-ARCHITECTURE.md` §4.1.

7. Deployable classes record: versioned id, approved image with immutable digest, architectures, licence/provenance, CPU/RAM/disk limits, terminals, configuration schema, **secret references** (never real secrets), volumes, health probes, rollout and placement policy, availability class (stateless / singleton / stateful).
8. Default to unprivileged containers in restricted projects; refuse broad host device or path mounts for ordinary modules. If a module needs privileges or kernel features a container cannot safely have (e.g. a general-purpose firewall), make it a VM.
9. Reject unsupported or mismatched terminal protocols and unresolved dependencies before deployment.
10. Keep reference fixtures for an nginx web tier, an API, a worker, PostgreSQL (stateful safety rules) and one optional VM.

### Adding a runtime (current process)

1. Schema: `ModuleClass.runtime` (`RuntimeSchema` in `packages/assembly-model/src/schema.ts`): `kind` (`incus-system-container`, `incus-oci-container`, `incus-vm`) and exactly one of `image` or `imageBy {property, images}`. `imageBy` must name an enum property and cover every option (`runtimeProblems`), so a deployment never runs something other than what the designer chose.
2. OCI images: `registry` + `reference` pinned by digest (`name@sha256:<64 hex>`), use the multi-architecture index digest and record the `version` it resolved to on the day. System/VM images: `server` + `alias`, optional `fingerprint` only from an image server that keeps old builds (public servers drop them).
2b. Settings: a class's settings change nothing unless its runtime uses them. An OCI runtime's `command` (the whole entry point, Incus `oci.entrypoint`) takes `{setting}` placeholders for number and choice settings with a default, and `{setting:yes|no}` for on/off ones; `environment` (`NAME: "text {setting}"`) takes any setting but a secret reference. Free text never goes into the command (Incus splits it like a shell). `runtimeProblems` enforces this; `appliedSettings` lists what is used, and the Studio marks every other setting of a deployable class as kept in the design. A deploy-time field (`supply: "deploy"`) reaches the instance as `SETUP_*`/`SECRET_*`, which an image does not read unless its runtime maps it. Map what the image really takes (flags or variables), and check it in the instance (`CHECKS` in `scripts/runtime-requests.ts`).
3. Put the runtime in `packages/module-catalog/src/runtimes.ts`, never inline in the class list.
4. Prove it on the disposable test cluster before committing: `npm run -s runtime-requests -w @apelora/module-catalog > /tmp/requests.json`, then `APELORA_TEST_CATALOG_REQUESTS=/tmp/requests.json cargo test -p apelora-deployment-worker --test real_incus -- --ignored catalog_runtimes` (see `docs/development/TEST-CLUSTER.md`). An image that has not run there stays design-only.
5. `instanceRequestFor(class, module, {project, name})` maps a placed module to the API's `InstanceSpec` (a test keeps the two types identical). Run `npm run catalog:doc` and `npm run schema` afterwards.

## Checklist

- Manifest, logo and (if relevant) template added
- Catalog validation passes
- Entry appears in the Studio menu under the right category and subcategory (the Playwright test "every catalog module drops onto the canvas…" adds every class and checks its logo, port order and properties form)

## Adding modules (current process)

1. Add the class in `packages/module-catalog/src/classes-extra.ts` with the shorthand helpers (`i` input, `o` output, `mon()` metrics, `ver/sec/num/txt/flag/pick` settings). Check the project's licence first and prefer open-source software; record source-available licences or cautions in the `upstream-reference` skill's catalog table.
1b. Place it in `packages/module-catalog/src/taxonomy.ts` (`CATALOG_TREE`: category → subcategory → ids, in menu order). The categories and subcategories themselves are the closed lists `CATEGORIES` and `SUBCATEGORIES` in `packages/assembly-model/src/constants.ts`; adding or renaming one is a deliberate change (rename: add the old name to `LEGACY_CATEGORIES` so saved files still import). Keep each subcategory to a handful of tiles (2–8, a test checks). A class missing from the tree fails at load.
1a. Describe its metrics in `packages/module-catalog/src/metrics.ts` from the project's own documentation, in our own words: source, port/path/format, on by default or localhost-only, how to turn on, access, health, signals, confidence (`unverified` when not confirmed). Name separate exporters, agents or collectors generically ("a separate exporter process on 9187"), never by project name. Give the class a `mon()` output **only** when `readableMetrics` holds (built in or exporter, metrics text, a port); a drawn `mon` connection opens exactly that port. Where the software has no default port, pick one, set `portChosen`, and say so in `enable`; where it depends on a version, use `portBy`. Rules and fields: `docs/architecture/MODULE-PORTS.md` ("Metrics"); tests: `src/metrics.test.ts`.
2. Follow the port placement rules in `docs/architecture/MODULE-PORTS.md`: service input first, `net` last on the left; primary downstream first, `mon` last on the right; short lowercase names (≤ 8 chars); at most 3 inputs and 4 outputs.
3. Icon: a Simple Icons mark (copy the SVG, add the brand fill, record file, product, source and licence in `assets/logos/ATTRIBUTION.md`) or a `generic-*` icon when there is no freely licensed mark. Read the mark's licence in Simple Icons' metadata: most are CC0-1.0, but some carry Apache-2.0, CC-BY(-SA) or custom trademark terms; use only CC0 or permissive (Apache-2.0, BSD) marks for new classes, otherwise a `generic-*` icon. A test checks every bundled mark is listed with a licence.
3b. Connections: a client output feeds a server input (the network compiler lets the output side reach the input side). Brokers have one client input (`in`) for producers and consumers; `peer`, `repl`, `primary` and `bridge` outputs must connect to the service input of another module of the same class (a test checks). Use the protocol's own type (`bgp`, `dns`, `imap`, …), not `tcp`, where one exists: a `tcp` output can only feed `tcp` inputs.
4. Run `npm test` (catalog-quality tests check schema, icons, placement, defaults) and `npm run catalog:doc` to regenerate `docs/architecture/CATALOG.md` and `packages/module-catalog/catalog-images.json` (the image servers of deployable classes, which the control plane compiles in for its image-mirror check, ADR 0010; a test fails when it is stale).
5. If a new protocol is needed, add it to `PORT_TYPES`, the layer-4 set in `compat.ts` (and its test), its well-known port in `DEFAULT_PORTS` (`packages/assembly-model/src/network.ts`), and a protocol cable colour in `apps/studio/src/components/canvas/cables.ts` (users may still override individual cables). `dns` (UDP 53), `bgp` (TCP 179) and `imap` (TCP 143) were added this way.
