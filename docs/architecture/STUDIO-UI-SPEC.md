# Apelora Studio — interface specification (front end only)

Everything here runs in the browser against mock data. Nothing is sent to a server or deployed. This is the reference for what the UI must do; `STUDIO-MVP.md` explains how it is built.

## 1. Layout

| Region | Spec |
|---|---|
| Header (48px) | Logo + "Apelora Studio", **Mock · Design only** badge, breadcrumb + save state (≥1400px), Undo/Redo, density (Compact / Comfortable / Expanded), New ▾ (empty, sample), Import, **Export JSON** (the single primary button), theme |
| Catalog (left, 176–192px) | Search, **New appliance**, collapsible category sections, 2-column tiles (logo + name). Tile: drag to canvas or click to add; pencil = edit custom / copy built-in |
| Canvas frame (centre) | Light-gray `#F7F7F8`, 12px radius, 1px border. Title strip: three dots, editable assembly name, toggles (grid, labels `Aa`, cable style, snap), zoom −/%/+/fit/zoom-to-selection. Validation panel docked bottom-left |
| Inspector (right, 224–256px) | Context-sensitive: assembly summary, appliance (name, class, size, resources, **Configuration**, ports, issues, actions), or cable |

## 2. Appliance node

128×96 flow units (compact), 24px header, centred 30px logo, left inputs / right outputs. Every terminal is an RJ45-style socket with a short label; connected sockets show a plug head in the protocol colour; `×N` marks several cables. Height grows only when ports need it. Status: amber/red border for warnings/errors. Resize handle bottom-right. Full geometry and behaviour: `STUDIO-MVP.md`.

## 3. Interactions

| Action | How | Result |
|---|---|---|
| Add appliance | Drag tile to canvas · click tile · canvas menu ▸ Add appliance | Lands under the pointer (drag/menu) or at the view centre without overlap (click); selected |
| Move / resize | Drag node / bottom-right grip | One undo step per gesture; size persists; density keeps manual sizes |
| Connect | Drag from a socket to a compatible socket (either direction) | Cable created; invalid → toast with reason and hint |
| Re-plug | Drag a cable's plug end to another socket | Validated; one undo step |
| Select | Click · Ctrl/⌘-click · Shift-drag marquee · Ctrl+A | Inspector follows selection |
| Delete | Del/Backspace · context menu | Appliances remove their cables; cables unplug |
| Copy / paste / duplicate | Ctrl+C / V / D, menus | Copies appliances + cables between them, fresh ids |
| Zoom / pan | Wheel, +/−, F, drag background, buttons | Viewport transform only |
| Undo / redo | Ctrl+Z, Ctrl+Shift+Z / Ctrl+Y, header | 100-step snapshot history |
| Import / export | Header | Validated, deterministic JSON; failure leaves the assembly untouched |

### Context menus (right-click, Menu key, Shift+F10)

Appliance, several appliances, port, cable and canvas menus are listed in `STUDIO-MVP.md`. Appliance menu additions in this milestone: **Configure…** (focuses the configuration form) and **Reset configuration** (shows how many fields changed). Keyboard: ↑/↓/Home/End, → opens a submenu, ← or Esc closes, Enter activates.

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

Dialog opened from **New appliance**, a tile's pencil, or **Save as custom appliance…**. Left: name, category, summary, vCPU, memory, generic icon, live node preview. Right, two tabs:

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

## 9. Out of scope (this phase)

Backend, deployment, real hosts, authentication, nested components (planned), setup parameters at application level, link tags, category colouring and orthogonal routing (proposed, not built).
