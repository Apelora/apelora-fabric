---
name: visual-designer
description: Build and modify Apelora Studio's visual canvas — React Flow nodes, resizable module containers, typed connectors, palette, inspector, and interactions. Use for any Vite/React/TypeScript UI work in Studio.
---

# Visual designer (Studio)

Stack: Vite, React, TypeScript (strict), React Flow (`@xyflow/react`).

## Principles

- The canvas is a view of the assembly document (see `application-assembly`). Store state as the document; derive React Flow nodes/edges from it, not the reverse.
- Module nodes render ports from the manifest. Handles carry the port `type` so invalid drops are rejected at connect time via `isValidConnection`, using shared logic from `topology-validation`.
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

## Deploying from the Studio (Phase 06)

- There is **no Deploy button in the canvas header**: deploying is done from the Applications list (`components/applications/DeployAction.tsx`, then `DeployVersionDialog`), which opens `components/deploy/DeployDialog.tsx` for a published version: connect (health, `whoami`, sign-in with a token kept in `sessionStorage`), configure (application name from `applicationName(metadata.name)`, project from `whoami.deployProjects`), review the server's plan, apply, follow the jobs.
- What is deployed comes from `deploymentModules(assembly, lookupClass, project)` (`@apelora/assembly-model`): modules whose class has a runtime; everything else is listed as design-only with the reason. Never send design-only modules.
- The plan shown is the server's (`POST /api/v1/applications/{name}/plan`); conflicts block applying. Applying sends the document, the deployable modules, `removeMissing` and an `Idempotency-Key`.
- Live state lives in `features/deploy/live.ts` (polls `…/status` every 5 s, refreshed right after jobs finish) and is shown as a dot in the node header (`data-testid="live-status"`, `data-state`); the header badge says where it comes from ("Live · N running in Incus" or "Mock data"). The overlay never changes the document.
- Tests: `tests/e2e/studio-deploy.spec.ts` stubs the API (unstubbed calls abort).
