---
name: react-flow-interactions
description: Implement or debug React Flow (@xyflow/react v12) behavior in Studio — controlled nodes/edges, drag-and-drop from the catalog with screenToFlowPosition, connecting with validation, delete, selection, undo gestures, fit and zoom. Use for any canvas interaction work.
---

# React Flow interactions

Source: `apps/studio/src/components/canvas/AssemblyWorkspace.tsx`. Check current API via the Context7 MCP or reactflow.dev before relying on memory (v12 names).

## Architecture

- Nodes/edges are **derived** from the zustand store (`useMemo`) and passed controlled. `onNodesChange`/`onEdgesChange` translate position, dimension (resize) and select changes into store actions; they ignore measurement-only dimension changes.
- Deletion goes through `onDelete` (one history entry), not individual remove changes.
- Drags and resizes are transient: `onNodeDragStart` → `beginGesture`, `onNodeDragStop` → `endGesture` (one undo step, none if nothing changed).
- Catalog drop: `screenToFlowPosition({x: clientX, y: clientY})`, minus half the default node size so the node centres under the cursor. Payload is only the class id (MIME `application/x-apelora-class`). Click-add uses the canvas centre and `findFreeSpot`.
- Connections: `connectionMode=Loose`, then `orient()` swaps an input→output drag into output→input; `isValidConnection` calls `explainConnection`; `onConnectEnd` shows the reason when a drop on a handle is rejected.
- Initial/after-import fit uses `fitView` with retry/`measured` so bounds are right; shortcuts: `+`, `-`, `f`, Ctrl+Z/Ctrl+Shift+Z/Ctrl+Y.

## Pitfalls seen here

- Controlled nodes without `measured` break `fitView` (only some nodes counted).
- Handles must be sized to the marker (9px) for edges to attach at the marker; widen only the hit area with a pseudo-element. Do not override the handle `transform`.
- Hover/focus styling must not use `transform` on handles.

## Acceptance

Specs for drag/drop at several zooms and after panning, connections (valid, wrong direction, type mismatch, duplicate), delete/undo/redo, and zoom shortcuts pass.
