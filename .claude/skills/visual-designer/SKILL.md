---
name: visual-designer
description: Build and modify Apelora Studio's visual canvas — React Flow nodes, resizable appliance containers, typed connectors, palette, inspector, and interactions. Use for any Vite/React/TypeScript UI work in Studio.
---

# Visual designer (Studio)

Stack: Vite, React, TypeScript (strict), React Flow (`@xyflow/react`).

## Principles

- The canvas is a view of the assembly document (see `application-assembly`). Store state as the document; derive React Flow nodes/edges from it, not the reverse.
- Appliance nodes render ports from the manifest. Handles carry the port `type` so invalid drops are rejected at connect time via `isValidConnection`, using shared logic from `topology-validation`.
- Resizing: use `NodeResizer` with min size from content; persist size in the document.
- Logos are local SVG assets only; never hotlink.
- Keyboard accessible: focusable nodes, delete/duplicate shortcuts, visible focus, labels on handles.
- Memoize custom node components and keep stable `nodeTypes`/`edgeTypes` objects outside render to avoid re-render storms.
- Undo/redo operates on document patches.

## Checklist

- No `any`; props typed from generated manifest types
- Invalid connections give visible feedback, not silent failure
- Large graphs (500+ nodes) remain interactive
- Component test plus a browser test (see `quality-assurance`) for each new interaction
- Verify in the running app before calling it done; this prototype is not a deployment engine, so do not imply it deploys anything
