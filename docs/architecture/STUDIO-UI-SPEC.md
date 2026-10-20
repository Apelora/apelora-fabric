# Apelora Studio — interface specification (front end only)

Everything here runs in the browser against mock data. Nothing is sent to a server or deployed. This is the reference for what the UI must do; `STUDIO-MVP.md` explains how it is built.

## 1. Layout

| Region | Spec |
|---|---|
| Header (48px) | Logo + "Apelora Studio", **Mock · Design only** badge, breadcrumb + save state (≥1400px), Undo/Redo, density (Compact / Comfortable / Expanded), New ▾ (empty, sample, sample + network), Import, **Export JSON** (the single primary button), theme |
| Catalog (left, 176–192px) | Search, appliance count with collapse all / expand all, collapsible category sections (Custom first, with **Create appliance…** at the end of the section), 2-column tiles (logo + name). Tile: drag to canvas or click to add; pencil = edit custom / copy built-in |
| Canvas frame (everything else) | Light-gray `#F7F7F8`, 12px radius, 1px border, **full width** (there is no permanent right sidebar). Title strip: three dots, **tools** (Select, Pan), editable assembly name, **Zones** menu, selection count, toggles (grid, labels `Aa`, cable style, routing, snap), zoom −/%/+/fit/zoom-to-selection. Validation panel docked bottom-left |
| Properties window (on demand) | A floating, draggable window opened from the context menu (**Properties…**), a double-click, or Enter. See "Properties window" below |

## 2. Appliance node

128×96 flow units (compact), 24px header, centred 30px logo, left inputs / right outputs. Every terminal is an RJ45-style socket with a short label; connected sockets show a plug head in the protocol colour; `×N` marks several cables. Height grows only when ports need it. Status: amber/red border for warnings/errors. Resize handle bottom-right. Full geometry and behaviour: `STUDIO-MVP.md`.

### Network interface (`net`)

Every appliance has a **net** terminal, the equivalent of a network card (NIC): an input of type `network`, single-cable, optional. It plugs into a **Network** appliance (a virtual segment, no VM) or any appliance that offers a `network` output (firewall `lan`, another Network). Left unplugged, the appliance joins the application's **default network**. A small **link light** on the jack turns green when the NIC is plugged; the inspector shows `Network: default network | <appliance> (cable) | <appliance> (tag lan)`. The `net` terminal cannot be deleted from an instance, is locked in the Appliance Designer (every custom appliance gets one), and is added automatically to older saved or imported assemblies.

### Link tags

A **link tag** names a terminal so it is connected, with no drawn cable, to every other terminal carrying the same name (an output feeds the inputs of its net). Use cables for the main data flow and tags for repetitive side links (monitoring, logs, the network). A tagged terminal shows its plug and a coloured name flag; hovering a flag highlights every member; clicking it selects the tag (inspector: rename/merge, member list, **Convert to cable** for one-output/one-input tags, **Remove tag**). Create: port row "link tag" field, port menu ▸ Link by name, or a cable's menu ▸ **Convert to link tag**. Rules match cables: protocol compatibility, no self-link, single-cable ports accept one link in total, required inputs are satisfied by a tag that has an output; nets without a source or listener raise warnings. Tags are not copied by Duplicate/Paste.

### Components (grouping)

Select two or more appliances and choose **Group into component…** (context menu or Ctrl+G). The dialog asks for a name and lists every inner terminal; terminals that are linked to something outside the selection are pre-selected and locked (marked "linked"), others can be exposed optionally, and each exposed port gets an editable name. Creating the component replaces the selection with one appliance, re-plugs the outside cables onto its exposed ports, registers a reusable custom class (catalog ▸ Custom) and shows a stacked-card look with a `▣ N` badge. Its `net` plugs every inner appliance's NIC into the network. **Ungroup** (menu, inspector button, Ctrl+Shift+G) expands it back into independent appliances and internal cables, re-plugging the outside cables to the inner terminals. Both are single undo steps. Exports embed the component class (and any custom classes it uses); importing restores it. Component classes are edited by ungrouping; they cannot be re-opened in the designer. Tags that cross the selection boundary stay with the inner appliances.

### Tools

- **Select (V, default):** click selects; **drag on empty canvas draws a selection box** (partial overlap selects); Shift- or Ctrl/⌘-click adds or removes; Ctrl+A selects all; clicking empty canvas clears. The strip shows how many appliances or zones are selected.
- **Pan (H):** dragging empty canvas moves the view. In Select mode you can pan with **Space + drag** or the **middle mouse button**; the wheel zooms.

### Properties window

Properties are **not** shown permanently. Open them with right-click ▸ **Properties…** (first menu item), double-click, or **Enter** on a selected appliance. The window floats near the thing it describes (and never off-screen), has a category-coloured header, can be dragged (its position is remembered), does not block the canvas, closes with ×, Esc, or when its target is deleted, and is retargeted by opening another one.

| Target | Content |
|---|---|
| Appliance | tabs **General** (name, class, id, network, size, resources, component contents, save as custom appliance, delete) · **Configuration (n)** (typed form) · **Ports (n)** (name, type, required, multi-cable, link tag, reorder) · **Issues (n)** |
| Cable | endpoints, protocol, convert to link tag, unplug |
| Link tag | rename or merge, members (click to select), convert to cable, remove |
| Zone / shape | tabs **Style** · **Label** · **Layout** (see Zones) |
| Assembly | counts and format (canvas menu ▸ Assembly properties…) |

Clicking an issue in the validation panel opens that appliance's **Issues** tab.

### Zones and shapes

Zones are labelled shapes drawn **behind** appliances to show sectors, trust zones or tiers. Four shapes: **square**, **circle**, **rounded square**, **triangle**.

- **Create:** Zones menu in the canvas strip (adds near the view centre, fanning out so zones never stack exactly), canvas right-click ▸ Add zone (at the pointer), or select appliances and choose ▸ **Wrap in zone** (sized to contain them, then the label editor opens).
- **Move / resize:** drag the zone's **border or label**; resize with the corner handles. The interior does not capture clicks, so cables and appliances under a zone stay usable and a selection box can start inside a zone. **Appliances inside a zone move with it**; the whole drag is one undo step. **Lock** pins a zone.
- **Edit (right-click ▸ Edit shape…, or double-click the label):** *Style* — shape kind, fill colour (8 modern palette colours plus a custom picker), fill opacity, border colour, border width, border style (solid, dashed, dotted), corner radius (rounded only). *Label* — text, position (top-left, top, centre, bottom), font size. *Layout* — x, y, width, height, lock, contents count, bring forward / send back, duplicate, delete. Each control merges rapid edits into a single undo step.
- **Right-click menu:** Edit shape, Edit label, Shape ▸, Colour ▸, Border colour ▸, Border style ▸, Select appliances inside, Bring forward, Send back, Duplicate, Lock, Delete.
- Zones are view-only layout (`editor.shapes`): they never change deployment semantics. Colours are validated as `#rrggbb`, sizes are bounded, and at most 200 zones are accepted on import. They scale with the density presets and are exported and imported with the assembly.

## 3. Interactions

| Action | How | Result |
|---|---|---|
| Add appliance | Drag tile to canvas · click tile · canvas menu ▸ Add appliance | Lands under the pointer (drag/menu) or at the view centre without overlap (click); selected |
| Move / resize | Drag node / bottom-right grip | One undo step per gesture; size persists; density keeps manual sizes |
| Connect | Drag from a socket to a compatible socket (either direction) | Cable created; invalid → toast with reason and hint |
| Re-plug | Drag a cable's plug end to another socket | Validated; one undo step |
| Select | Click · Shift/Ctrl-click · drag a box · Ctrl+A | Count shown in the canvas strip; nothing opens automatically |
| Delete | Del/Backspace · context menu | Appliances remove their cables; cables unplug |
| Copy / paste / duplicate | Ctrl+C / V / D, menus | Copies appliances + cables between them, fresh ids |
| Zoom / pan | Wheel, +/−, F, drag background, buttons | Viewport transform only |
| Undo / redo | Ctrl+Z, Ctrl+Shift+Z / Ctrl+Y, header | 100-step snapshot history |
| Import / export | Header | Validated, deterministic JSON; failure leaves the assembly untouched |

### Context menus (right-click, Menu key, Shift+F10)

Appliance, several appliances, port, cable and canvas menus are listed in `STUDIO-MVP.md`. Appliance menu: **Properties…** (Enter), Rename (F2), **Configure…** (opens the Configuration tab) and **Reset configuration** (shows how many fields changed). Keyboard: ↑/↓/Home/End, → opens a submenu, ← or Esc closes, Enter activates.

## 4. Configuration forms

Appliance classes declare typed **properties**; each instance stores only **overrides** (`config`), so defaults are never copied and "reset" simply removes the override.

| Type | Control | Validation |
|---|---|---|
| string | text | ≤ 256 chars |
| int | number | integer, optional min/max |
| bool | toggle | — |
| enum | select | must be one of the options |
| secret | text, monospace | a **reference name** (`db-password`), never the secret; a pasted value fails with "Never paste the secret itself" |

Fields are grouped (first group open; groups with changed or invalid fields open automatically), show a • marker and a per-field **reset** when changed, **Reset all** in the header, inline `role="alert"` errors with a correction hint, and mark required fields with `*`. Invalid values are kept (so the user sees what is wrong), flag the node red, and appear in the validation panel as `config-invalid`. Class property ids are stable: editing a custom class keeps ids so saved values keep matching.

## 5. Appliance Designer (custom appliances)

Dialog opened from **Create appliance…** in the Custom section, a tile's pencil, or **Save as custom appliance…**. Left: name, category, summary, vCPU, memory, generic icon, live node preview. Right, two tabs:

- **Ports** — name (short, shown on the canvas), protocol type, description (tooltip), required, several-cables, reorder, delete.
- **Configuration** — fields with label, type, default, group, help text, required; enum options (comma separated), int min/max. Saving validates labels, enum option count, min ≤ max and that each default satisfies its own rules; errors list in a red alert and nothing is saved until fixed.

"Save as custom appliance…" copies the instance's ports and uses its *current* configuration values as the new class defaults. Custom classes live in `localStorage`, are embedded in exported assemblies, and are restored on import.

## 6. Motion

Tokens: `--apl-dur-fast 120ms`, `--apl-dur 180ms`, `--apl-dur-slow 260ms`, `--apl-ease cubic-bezier(.2,.7,.2,1)`; the same values are in `features/editor/motion.ts`.

| Element | Transition |
|---|---|
| Context menu / submenu | pop (opacity + scale .96→1, 120ms) from the top-left |
| Dialogs | box pops, backdrop fades (180ms) |
| Toasts | rise + fade (180ms) |
| Node added | fade in (180ms; opacity only so geometry stays exact) |
| Plug connected | slides in from outside the socket (180ms) |
| Cable created | draws along its length (260ms); label and arrow fade in |
| Hover / selection | node border and shadow, jack colour, cable width (120ms) |
| Catalog section open, inspector group open, validation list | drop-in fade (120ms) |
| Zoom, fit, zoom-to-selection | animated viewport (120–250ms) |

**Reduced motion:** when `prefers-reduced-motion: reduce`, CSS durations collapse to ~0 and programmatic viewport animations use duration 0. Nothing depends on an animation finishing.

## 7. Accessibility

Every control has an accessible name; sockets expose name, type, required, cable policy and plugged count via `aria-label`; menus use `role=menu/menuitem(checkbox)` with roving focus; errors use `role=alert`; focus is visible; colour is never the only signal (labels, ×N, text errors); contrast follows daisyUI themes (light, winter, night).

## 8. States and errors

Empty canvas hint; validation panel (No issues / counts / list with hints, click focuses the appliance); import failure dialog with the first errors; corrupted autosave is kept under `apelora.studio.v1.invalid` and the user is told; storage unavailable degrades silently to in-memory.

## 8a. Workflow aids

- **Connect faster:** right-click a jack → *Add appliance that connects here* lists compatible catalog appliances (exact protocol first) and places and cables one. While a cable is dragged, compatible sockets light up and the rest dim.
- **Starting points:** *New → From template…* (nine starter diagrams built from the catalog) and *From Docker Compose…* (services become appliances, `depends_on` becomes cables; the file is read as text only).
- **Layout:** *Tidy* (Shift+T) arranges the flow left to right and re-fits zones; alignment guides and snapping while dragging; arrow keys nudge (Shift = 40); overview map; Find (Ctrl+K); only visible elements are rendered above 150 appliances.
- **Documentation:** sticky notes; vCPU/memory totals for the assembly and per zone; export as SVG, PNG or print; share link (the diagram is packed into the URL fragment, nothing is uploaded).
- **Safety:** named snapshots with a plain-language comparison; share links keep the previous diagram as a snapshot.
- **Target platform:** the assembly records a design target (Incus by default; Xen and VMware planned) in `spec.target.platform`; see `HYPERVISOR-PROVIDERS.md`.
- **Help:** `?` opens the shortcut sheet; a four-step tour appears once on a first visit.

## 9. Out of scope (this phase)

Backend, deployment, real hosts, authentication, drilling into a component on the canvas (contents are listed in the inspector), validating a component's inner assembly, setup parameters at application level.
