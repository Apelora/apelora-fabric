# Apelora Studio — MVP architecture (frontend only)

Status: implemented as milestone M1/M2/M3 in `apps/studio`; backend work has **not** started.

See also [STUDIO-UI-SPEC.md](STUDIO-UI-SPEC.md) for the interface specification (layout, interactions, configuration forms, designer, motion, accessibility).

## Scope

A browser application to compose, validate, inspect, version and export application assemblies from a catalog of virtual modules. Everything is local: state lives in memory and `localStorage`; nothing is deployed.

## Layout

```
apps/studio/            Vite + React app
  src/components/{chrome,catalog,canvas,inspector,validation}
  src/features/editor/  zustand store, persistence, context
packages/assembly-model schema, geometry, validation, ops, serialize
packages/module-catalog classes, logos, sample assembly
tests/e2e/              Playwright specs (+ on-demand screenshots)
```

## Layers

| Layer | Type | Contains |
|---|---|---|
| Catalog | `ModuleClass` | id, version, category, ports (stable ids), resources, icon, runtime image placeholder |
| Instance | `ModuleInstance` | id, name, `classRef`, resources, own copy of ports (editable) |
| Assembly | `ApplicationAssembly` | `spec` (modules, connections) + `editor` (layout, density) |

`spec` is deployment-relevant; `editor` is view-only (positions, sizes, density) so resizing never changes semantics. The JSON Schema is generated from the zod schemas into `packages/assembly-model/schema/` for future Rust consumers.

## State and history

One zustand store holds the whole assembly (immutable). History is a snapshot stack (max 100) using structural sharing. Continuous gestures (drag, resize) are transient and collapse into one undo step (`beginGesture` / `endGesture`). React Flow nodes and edges are **derived** from the store each render (controlled mode), so there is no duplicated state.

## Geometry contract

- Default node 128×96 flow units; header 24; logo 30; port pitch 18; marker 9, hit area 18.
- `minModuleHeight = max(96, 60 + 18 × rows)` where rows = max(1, inputs, outputs). Two ports fit in the compact card; three or more grow the node only.
- Port i centre: `24 + 18 + 18·i`.
- Density presets: compact 128×96, comfortable 152×112, expanded 178×150.
- **Density vs manual size:** nodes the user has not resized follow the preset; a manually resized node keeps its own size (`custom: true`) and can be reset from the inspector. Heights never drop below the port minimum. Positions scale with the preset so spacing is preserved. (Tested.)

## Terminals, labels and custom modules

- Every terminal is an RJ45-style **socket** (hollow, latch notch, contact pins); inputs are on the left, outputs on the right. The 10px jack has an 18px pointer target.
- **Cable style.** The default is a thin 1.6px coloured line with a small plug head at each end (clean at any density); a toolbar toggle switches to a heavier outlined patch-cable look.
- **Cables plug in.** A connected socket shows an RJ45 plug head inserted into it, and the cable (muted colour by protocol: http blue, postgres indigo, redis red, …) leaves from the plug's strain relief with a small flow arrow near its target. Plug heads are rendered inside the node so they sit above the socket; the cable starts 6px beyond the jack edge, under the boot. Unplugged sockets stay hollow. **Cable colour:** a user can give any cable its own colour (Cable properties or right-click → Cable colour) and reset it to the protocol colour; the choice is layout only (`editor.cables`), survives export/import and undo, is dropped when the cable is unplugged, and an invalid cable is always shown red. Plug heads keep the protocol colour because a port can carry several cables.
- **Single vs multiple cables.** Each port has a multiplicity. `multi` (the default for new ports) accepts several cables into the same socket, shown as one plug with a `×N` count; `single` accepts one and rejects another with a reason. Toggle it per port in the inspector or the Module Designer.
- **Re-plugging.** Drag a cable's plug end to another compatible socket to move it (validated; one undo step).
- Every port shows a short text label inside the node next to its jack (truncated to the free space beside the logo, never over it); the full name, type and description appear in a tooltip. Links show the carried protocol as a small label. The `Aa` toggle in the canvas strip hides both.
- **Module Designer** (Create module… in the Custom section, or the pencil on a tile, or "Save as custom module…" in the inspector): name, category, summary, generic icon, resources and typed ports with a live preview. Custom classes persist in `localStorage` (`apelora.studio.catalog.v1`), appear in the catalog, and are embedded in exported assemblies (`catalog.classes`); importing an assembly registers any missing ones.
- Editing a custom class does not change existing instances (they hold their own port copies); deleting is blocked while the current assembly uses it.

## Tools, properties window, zones

Select/Pan tool modes (`tool` in the store; React Flow `selectionOnDrag`/`panOnDrag`), an on-demand floating properties window (`PropertiesWindow.tsx`, store `propsWin`/`propsPos`) replacing the old sidebar, and zones (`editor.shapes`, `packages/assembly-model/src/shapes.ts`, `ShapeNode.tsx`, `ShapeProperties.tsx`). Shapes are React Flow nodes with `zIndex: -1` and `elevateNodesOnSelect` off so they stay behind modules; only their border and label take pointer events. Dragging a zone moves the modules whose centre lies inside it (`shapeContents`, `carry` in the store). Style edits use `commitCoalesced` so slider drags and typing form one undo step. Specified in `STUDIO-UI-SPEC.md`.

## Context menus and shortcuts

Right-click (or the Menu key / Shift+F10 on a selection) opens a context menu; arrow keys, Enter, Right/Left (submenus) and Escape work, and it closes on outside click, scroll or resize. Right-clicking something outside the current selection selects it first.

| Target | Actions |
|---|---|
| Module | Rename (F2), Duplicate (Ctrl+D), Copy (Ctrl+C), Add port ▸ input/output, Reset size, Unplug all cables, Zoom to module, Save as custom module…, Edit module class… (custom only), Delete |
| Several modules | Duplicate, Copy, Align ▸ (left, right, top, bottom, centre H/V), Zoom to selection, Delete N |
| Port (jack) | Rename port…, Protocol type ▸, Allow several cables ✓, Required ✓, Unplug cables (n), Move up/down, Delete port |
| Cable | Select source / target module, Unplug cable |
| Canvas | Add module ▸ (by category, at the pointer), Paste (at the pointer), Select all, Undo/Redo, Fit to view, Dotted grid ✓, Port and link labels ✓, Thick patch-cable look ✓ |

Duplicate/Paste copy the modules and only the cables **between** the copied modules, with fresh ids; names default to the new id. Shortcuts: Ctrl+C/V/D/A, F2, Delete, Ctrl+Z / Ctrl+Shift+Z / Ctrl+Y, `+` `-` `F`.

## Category colours, routing, link tags, network interface

- Node headers carry a muted category accent (and a small class-name footer); catalog tiles and section headings use the same colours. Toggle in the canvas menu; persisted with the other view preferences (grid, labels, cable style, routing, snap) in `localStorage` (`apelora.studio.view.v1`).
- Cables can be **curved** or **right-angle** (canvas header or menu).
- Link tags and the `net` interface are specified in `STUDIO-UI-SPEC.md`. Model: `spec.links[]` (`{id, tag, endpoint}`); logic in `packages/assembly-model/src/tags.ts` (`tagPort`, `cableToTag`, `tagToCable`, `explainTag`, `linkCount`), shared with validation so cables and tags obey the same multiplicity rules. `ensureNics` migrates older assemblies.

## Connections

Validated by one pure function (`explainConnection`) used for live feedback and whole-assembly validation: direction, self-connection, port existence, type compatibility, duplicates, multiplicity. Users may drag from either end; the pair is oriented output → input first. Rejections show a reason and a hint. Required inputs that are unconnected are reported as errors in the validation panel (amber on the node while it is only that).

## Persistence and files

- Autosave to `localStorage` (debounced 400 ms) as `{version: 1, assembly: <serialized text>}`. If stored data fails validation it is **kept as a backup** (`apelora.studio.v1.invalid`) and the user is told; it is never silently overwritten.
- Export: deterministic JSON (sorted keys, collections sorted by id). Import: size-capped, schema-validated, structurally checked; failure leaves the current assembly untouched; success is undoable.

## Not in this milestone

Control Console (M5), Rust API boundary (M6), Incus integration and HA (M7; see `docs/development/IMPLEMENTATION-PHASES.md`), authentication, real deployment, planning against real hosts, runtime image resolution, health policies, nested assemblies (compose a selection into a class), setup parameters, copy/paste, minimap.

## Known limitations

- Density changes scale node positions proportionally to preserve spacing; manually resized nodes keep their size, so a very large custom node can still overlap neighbours. Round trips may drift by a pixel from rounding.
- Port rename/reorder is done in the inspector, not directly on the node.
- Logos are simple-icons marks (CC0 artwork, trademarks remain with owners); HAProxy, gateway and Linux base use original neutral icons.
