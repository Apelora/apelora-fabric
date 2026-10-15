---
name: compact-appliance
description: Build or change the Apelora appliance node — 128×96 compact card, 24px header, centered 30px logo, left inputs/right outputs, automatic height growth, resizing, density presets. Use when touching the node component, handle layout, or node sizing rules.
---

# Compact appliance node

Source: `apps/studio/src/components/canvas/ApplianceNode.tsx`, geometry in `packages/assembly-model/src/geometry.ts`.

## Geometry (flow units, independent of zoom)

`NODE = {width 128, height 96, header 24, logo 30, portPitch 18}`; marker 9, hit area 18.
- `minApplianceHeight(i, o) = max(96, 60 + 18·max(1,i,o))`; port centre `y = 42 + 18·index`.
- Density: compact 128×96, comfortable 152×112, expanded 178×150. Manually resized nodes (`custom`) keep their size; others follow the preset. Heights never go below the port minimum.
- Adding ports grows **only that node's height**, never its width or other nodes.

## Rules

1. Logo centered; port labels never sit on it: names appear in a hover tooltip outside the node and in `title`/`aria-label`; full editing is in the inspector.
2. Handle ids are port ids. After any port or size change `useUpdateNodeInternals(id)` must run (the node does this from a signature of ports and size).
3. Provide `measured` size on derived nodes so React Flow's fit/bounds are correct in controlled mode.
4. Resizing uses `NodeResizeControl` bottom-right with `minWidth 128`, `minHeight = minHeightForPorts`; begin/end call `beginGesture/endGesture` so it is one undo step.
5. Never use CSS zoom; zoom is the viewport transform.

## Acceptance

Playwright specs "compact nodes are 128×96…", "adding 3–8 ports grows only the affected node", and "moving, resizing and density changes keep edges anchored" pass. Edge anchoring is checked numerically against handle bounding boxes (±1.5 flow px).
