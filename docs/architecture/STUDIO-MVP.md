# Apelora Studio — MVP architecture (frontend only)

Status: implemented as milestone M1/M2/M3 in `apps/studio`; backend work has **not** started.

## Scope

A browser application to compose, validate, inspect, version and export application assemblies from a catalog of virtual appliances. Everything is local: state lives in memory and `localStorage`; nothing is deployed.

## Layout

```
apps/studio/            Vite + React app
  src/components/{chrome,catalog,canvas,inspector,validation}
  src/features/editor/  zustand store, persistence, context
packages/assembly-model schema, geometry, validation, ops, serialize
packages/appliance-catalog classes, logos, sample assembly
tests/e2e/              Playwright specs (+ on-demand screenshots)
```

## Layers

| Layer | Type | Contains |
|---|---|---|
| Catalog | `ApplianceClass` | id, version, category, ports (stable ids), resources, icon, runtime image placeholder |
| Instance | `ApplianceInstance` | id, name, `classRef`, resources, own copy of ports (editable) |
| Assembly | `ApplicationAssembly` | `spec` (appliances, connections) + `editor` (layout, density) |

`spec` is deployment-relevant; `editor` is view-only (positions, sizes, density) so resizing never changes semantics. The JSON Schema is generated from the zod schemas into `packages/assembly-model/schema/` for future Rust consumers.

## State and history

One zustand store holds the whole assembly (immutable). History is a snapshot stack (max 100) using structural sharing. Continuous gestures (drag, resize) are transient and collapse into one undo step (`beginGesture` / `endGesture`). React Flow nodes and edges are **derived** from the store each render (controlled mode), so there is no duplicated state.

## Geometry contract

- Default node 128×96 flow units; header 24; logo 30; port pitch 18; marker 9, hit area 18.
- `minApplianceHeight = max(96, 60 + 18 × rows)` where rows = max(1, inputs, outputs). Two ports fit in the compact card; three or more grow the node only.
- Port i centre: `24 + 18 + 18·i`.
- Density presets: compact 128×96, comfortable 152×112, expanded 178×150.
- **Density vs manual size:** nodes the user has not resized follow the preset; a manually resized node keeps its own size (`custom: true`) and can be reset from the inspector. Heights never drop below the port minimum. Positions scale with the preset so spacing is preserved. (Tested.)

## Connections

Validated by one pure function (`explainConnection`) used for live feedback and whole-assembly validation: direction, self-connection, port existence, type compatibility, duplicates, multiplicity. Users may drag from either end; the pair is oriented output → input first. Rejections show a reason and a hint. Required inputs that are unconnected are reported as errors in the validation panel (amber on the node while it is only that).

## Persistence and files

- Autosave to `localStorage` (debounced 400 ms) as `{version: 1, assembly: <serialized text>}`. If stored data fails validation it is **kept as a backup** (`apelora.studio.v1.invalid`) and the user is told; it is never silently overwritten.
- Export: deterministic JSON (sorted keys, collections sorted by id). Import: size-capped, schema-validated, structurally checked; failure leaves the current assembly untouched; success is undoable.

## Not in this milestone

Control Console (M5), Rust API boundary (M6), Xen/Oxia/HA (M7), authentication, real deployment, planning against real hosts, runtime image resolution, health policies, nested assemblies, copy/paste, minimap.

## Known limitations

- Density changes scale node positions proportionally to preserve spacing; manually resized nodes keep their size, so a very large custom node can still overlap neighbours. Round trips may drift by a pixel from rounding.
- Port rename/reorder is done in the inspector, not directly on the node.
- Logos are simple-icons marks (CC0 artwork, trademarks remain with owners); HAProxy, gateway and Linux base use original neutral icons.
