import { useCallback, useEffect, useMemo, useRef } from "react";
import {
  Background, BackgroundVariant, ConnectionMode, MiniMap, ReactFlow, SelectionMode, ViewportPortal, useReactFlow, useStore as useRFStore,
  type Connection, type Edge, type NodeChange, type EdgeChange, type OnConnectEnd, type OnConnectStart,
} from "@xyflow/react";
import { logoUrl, lookupClass } from "@apelora/module-catalog";
import { DENSITY_SIZE, alignmentGuides, cableColorOf, effectiveSize, explainConnection, minHeightForPorts, portOf, tagNet, type Diagnostic, type ApplicationAssembly, type Endpoint } from "@apelora/assembly-model";
import { useStudio, useStudioApi } from "../../features/editor/context";
import { diagnosticsFor } from "../../features/editor/store";
import { canvasBridge, CATALOG_MIME } from "../../features/editor/bridge";
import { useCustomCatalog } from "../../features/catalog/customCatalog";
import { ModuleNode, type ModuleFlowNode, type ModuleNodeData } from "./ModuleNode";
import { ShapeNode, type ShapeFlowNode } from "./ShapeNode";
import { CableEdge, type CableFlowEdge } from "./CableEdge";
import { CanvasStrip } from "./CanvasStrip";
import { ValidationPanel } from "../validation/ValidationPanel";
import { ContextMenu } from "./ContextMenu";
import { ms } from "../../features/editor/motion";

const nodeTypes = { module: ModuleNode, shape: ShapeNode };
type AnyFlowNode = ModuleFlowNode | ShapeFlowNode;
const edgeTypes = { cable: CableEdge };

/** How many cables are plugged into each port of a module, and the protocol of the first one. */
function linksFor(a: ApplicationAssembly, id: string): Record<string, { count: number; type: string }> {
  const out: Record<string, { count: number; type: string }> = {};
  for (const c of a.spec.connections) {
    const end = c.source.moduleId === id ? c.source.portId : c.target.moduleId === id ? c.target.portId : null;
    if (!end) continue;
    const prev = out[end];
    out[end] = prev ? { ...prev, count: prev.count + 1 } : { count: 1, type: linkLabel(a, c.source) };
  }
  return out;
}

/** Link tags on a module's ports, with the protocol of each net (taken from its first output). */
function tagsFor(a: ApplicationAssembly, id: string): Record<string, { name: string; type: string; members: number }> {
  const out: Record<string, { name: string; type: string; members: number }> = {};
  for (const l of a.spec.links ?? []) {
    if (l.endpoint.moduleId !== id) continue;
    const net = tagNet(a, l.tag);
    const src = net.map((m) => portOf(a, m.endpoint)).find((p) => p?.direction === "output") ?? portOf(a, l.endpoint);
    out[l.endpoint.portId] = { name: l.tag, type: src?.type ?? "tcp", members: net.length };
  }
  return out;
}

/** Link label = the protocol carried by the source terminal. */
function linkLabel(a: ApplicationAssembly, s: Endpoint): string {
  return a.spec.modules.find((x) => x.id === s.moduleId)?.ports.find((p) => p.id === s.portId)?.type ?? "";
}

/** Lets users drag from either side: orient the pair output → input before validating. */
function orient(a: ApplicationAssembly, c: Pick<Connection, "source" | "target" | "sourceHandle" | "targetHandle">): { source: Endpoint; target: Endpoint } | null {
  if (!c.source || !c.target || !c.sourceHandle || !c.targetHandle) return null;
  const s: Endpoint = { moduleId: c.source, portId: c.sourceHandle };
  const t: Endpoint = { moduleId: c.target, portId: c.targetHandle };
  const sp = a.spec.modules.find((x) => x.id === s.moduleId)?.ports.find((p) => p.id === s.portId);
  const tp = a.spec.modules.find((x) => x.id === t.moduleId)?.ports.find((p) => p.id === t.portId);
  if (sp?.direction === "input" && tp?.direction === "output") return { source: t, target: s };
  return { source: s, target: t };
}

export function AssemblyWorkspace() {
  const api = useStudioApi();
  const assembly = useStudio((s) => s.assembly);
  const selection = useStudio((s) => s.selection);
  const showGrid = useStudio((s) => s.showGrid);
  const showLabels = useStudio((s) => s.showLabels);
  const cableStyle = useStudio((s) => s.cableStyle);
  const routing = useStudio((s) => s.routing);
  const tool = useStudio((s) => s.tool);
  const showCategory = useStudio((s) => s.showCategory);
  const customClasses = useCustomCatalog((s) => s.classes);
  const snap = useStudio((s) => s.snap);
  const minimap = useStudio((s) => s.minimap);
  const guides = useStudio((s) => s.guides);
  const fitNonce = useStudio((s) => s.fitNonce);
  const { screenToFlowPosition, fitView } = useReactFlow();
  const frameRef = useRef<HTMLDivElement>(null);

  useEffect(() => { canvasBridge.el = frameRef.current; return () => { canvasBridge.el = null; }; }, []);
  const didInitialFit = useRef(false);
  useEffect(() => {
    // Fit once the first nodes are measured, so the initial viewport is correct (retry for a few frames).
    let frames = 0;
    let raf = 0;
    const tryFit = () => {
      if (didInitialFit.current || frames++ > 40) return;
      void fitView({ padding: 0.2, maxZoom: 1.2 }).then((ok) => { if (ok) didInitialFit.current = true; else raf = requestAnimationFrame(tryFit); });
    };
    raf = requestAnimationFrame(tryFit);
    return () => cancelAnimationFrame(raf);
  }, [fitView]);
  useEffect(() => {
    if (fitNonce > 0) { const t = setTimeout(() => void fitView({ padding: 0.2, maxZoom: 1.2, duration: ms(250) }), 60); return () => clearTimeout(t); }
  }, [fitNonce, fitView]);

  // Validation only depends on the spec, so dragging nodes (which only changes the layout) does not re-run it.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const diagnostics = useMemo(() => diagnosticsFor(assembly), [assembly.spec, customClasses]);

  const shapeNodes = useMemo<ShapeFlowNode[]>(() => (assembly.editor.shapes ?? []).map((sh) => ({
    id: sh.id,
    type: "shape" as const,
    position: { x: sh.x, y: sh.y },
    width: sh.width,
    height: sh.height,
    measured: { width: sh.width, height: sh.height },
    zIndex: -1,
    draggable: !sh.locked,
    selected: (selection.shapes ?? []).includes(sh.id),
    data: { shape: sh, inside: 0 },
  })), [assembly.editor.shapes, selection.shapes]);

  // Per-module data that comes from the spec (ports, links, tags, status). Recomputed only when the spec changes.
  const specData = useMemo(() => {
    const byApp = new Map<string, Diagnostic[]>();
    for (const d of diagnostics) if (d.moduleId) byApp.set(d.moduleId, [...(byApp.get(d.moduleId) ?? []), d]);
    const out = new Map<string, Record<string, unknown>>();
    for (const a of assembly.spec.modules) {
      const mine = byApp.get(a.id) ?? [];
      const hard = mine.some((d) => d.severity === "error" && d.code !== "required-port-unconnected");
      const cls = lookupClass(a.classRef.name, a.classRef.version);
      out.set(a.id, {
        name: a.name,
        className: cls?.displayName ?? a.classRef.name,
        category: cls?.category ?? "Custom",
        inner: cls?.composition?.modules.length ?? 0,
        icon: cls ? logoUrl(cls.iconAssetPath) : undefined,
        inputs: a.ports.filter((p) => p.direction === "input"),
        outputs: a.ports.filter((p) => p.direction === "output"),
        minHeight: minHeightForPorts(a.ports),
        status: hard ? "error" : mine.length ? "warning" : "ok",
        links: linksFor(assembly, a.id),
        tags: tagsFor(assembly, a.id),
      });
    }
    return out;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [assembly.spec, diagnostics, customClasses]);

  // React Flow skips work for nodes whose object is unchanged, so reuse each node object while its inputs are the same.
  const nodeCache = useRef(new Map<string, { base: unknown; x: number; y: number; w: number; h: number; sel: boolean; node: ModuleFlowNode }>());
  const moduleNodes = useMemo<ModuleFlowNode[]>(() => {
    const layout = new Map(assembly.editor.nodes.map((n) => [n.id, n]));
    const next = new Map<string, { base: unknown; x: number; y: number; w: number; h: number; sel: boolean; node: ModuleFlowNode }>();
    const list = assembly.spec.modules.flatMap((a) => {
      const l = layout.get(a.id);
      const base = specData.get(a.id);
      if (!l || !base) return [];
      const size = effectiveSize(l, a.ports, assembly.editor.density);
      const sel = selection.nodes.includes(a.id);
      const hit = nodeCache.current.get(a.id);
      if (hit && hit.base === base && hit.x === l.x && hit.y === l.y && hit.w === size.width && hit.h === size.height && hit.sel === sel) { next.set(a.id, hit); return [hit.node]; }
      const node: ModuleFlowNode = {
        id: a.id, type: "module" as const, position: { x: l.x, y: l.y }, width: size.width, height: size.height, measured: { width: size.width, height: size.height }, selected: sel,
        data: { ...base, width: size.width, height: size.height } as ModuleNodeData,
      };
      next.set(a.id, { base, x: l.x, y: l.y, w: size.width, h: size.height, sel, node });
      return [node];
    });
    nodeCache.current = next;
    return list;
  }, [assembly.spec, assembly.editor.nodes, assembly.editor.density, specData, selection.nodes]);
  const nodes = useMemo<AnyFlowNode[]>(() => [...shapeNodes, ...moduleNodes], [shapeNodes, moduleNodes]);

  const edges = useMemo<CableFlowEdge[]>(() => {
    const bad = new Set(diagnostics.filter((d) => d.connectionId).map((d) => d.connectionId));
    return assembly.spec.connections.map((c) => ({
      id: c.id,
      type: "cable" as const,
      source: c.source.moduleId,
      sourceHandle: c.source.portId,
      target: c.target.moduleId,
      targetHandle: c.target.portId,
      selected: selection.edges.includes(c.id),
      reconnectable: true,
      data: { type: linkLabel(assembly, c.source), color: cableColorOf(assembly, c.id), invalid: bad.has(c.id), angled: routing === "angled", ...(showLabels ? { label: linkLabel(assembly, c.source) } : {}) },
    }));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [assembly.spec, assembly.editor.cables, diagnostics, selection.edges, showLabels, routing]);

  const onNodesChange = useCallback((changes: NodeChange<AnyFlowNode>[]) => {
    const s = api.getState();
    const moves = new Map<string, { x: number; y: number }>();
    const shapeMoves = new Map<string, { x: number; y: number }>();
    let selNodes = s.selection.nodes;
    let selShapes = s.selection.shapes ?? [];
    let selChanged = false;
    for (const ch of changes) {
      const isShape = ch.type !== "add" && ch.type !== "replace" && ch.type !== "remove" ? ch.id.startsWith("shape-") : false;
      if (ch.type === "position" && ch.position) {
        if (isShape) {
          shapeMoves.set(ch.id, ch.position);
          // Modules inside a dragged zone travel with it.
          const cur = (s.assembly.editor.shapes ?? []).find((x) => x.id === ch.id);
          const carry = s.carry;
          if (cur && carry?.shapeId === ch.id) {
            const dx = ch.position.x - cur.x;
            const dy = ch.position.y - cur.y;
            for (const nid of carry.ids) { const n = s.assembly.editor.nodes.find((x) => x.id === nid); if (n && !moves.has(nid)) moves.set(nid, { x: n.x + dx, y: n.y + dy }); }
          }
        } else moves.set(ch.id, ch.position);
      } else if (ch.type === "dimensions" && ch.dimensions && (ch.resizing || ch.setAttributes)) {
        if (isShape) {
          const pos = changes.find((c) => c.type === "position" && c.id === ch.id);
          s.resizeShapeTransient(ch.id, { width: ch.dimensions.width, height: ch.dimensions.height, ...(pos && pos.type === "position" && pos.position ? { x: pos.position.x, y: pos.position.y } : {}) });
        }
        else s.resizeTransient(ch.id, ch.dimensions);
      } else if (ch.type === "select") {
        selChanged = true;
        if (isShape) selShapes = ch.selected ? [...new Set([...selShapes, ch.id])] : selShapes.filter((x) => x !== ch.id);
        else selNodes = ch.selected ? [...new Set([...selNodes, ch.id])] : selNodes.filter((x) => x !== ch.id);
      }
    }
    // A single module being dragged snaps to the edges and centres of its neighbours.
    if (moves.size === 1 && shapeMoves.size === 0 && !s.carry) {
      const [id, p] = [...moves][0]!;
      const g = alignmentGuides(s.assembly, id, p);
      moves.set(id, g.pos);
      s.setGuides(g.guides);
    }
    if (shapeMoves.size) s.moveShapesTransient(shapeMoves);
    if (moves.size) s.moveTransient(moves);
    if (selChanged) s.setSelection({ nodes: selNodes, shapes: selShapes });
  }, [api]);

  const onEdgesChange = useCallback((changes: EdgeChange<CableFlowEdge>[]) => {
    let sel = api.getState().selection.edges;
    let changed = false;
    for (const ch of changes) {
      if (ch.type === "select") { changed = true; sel = ch.selected ? [...new Set([...sel, ch.id])] : sel.filter((x) => x !== ch.id); }
    }
    if (changed) api.getState().setSelection({ edges: sel });
  }, [api]);

  const isValidConnection = useCallback((c: Connection | Edge) => {
    const a = api.getState().assembly;
    const o = orient(a, c as Connection);
    return !!o && explainConnection(a, o.source, o.target) === null;
  }, [api]);

  const onReconnect = useCallback((old: Edge, c: Connection) => {
    const o = orient(api.getState().assembly, c);
    if (o) api.getState().reconnectCable(old.id, o.source, o.target);
  }, [api]);

  const onConnect = useCallback((c: Connection) => {
    const o = orient(api.getState().assembly, c);
    if (o) api.getState().connectPorts(o.source, o.target);
  }, [api]);

  const onConnectStart = useCallback<OnConnectStart>((_e, p) => {
    if (!p.nodeId || !p.handleId) return;
    const port = api.getState().assembly.spec.modules.find((x) => x.id === p.nodeId)?.ports.find((q) => q.id === p.handleId);
    if (port) api.getState().setDragFrom({ moduleId: p.nodeId, portId: port.id, direction: port.direction, type: port.type });
  }, [api]);

  const onConnectEnd = useCallback<OnConnectEnd>((_e, state) => {
    api.getState().setDragFrom(null);
    if (state.isValid !== false || !state.fromHandle || !state.toHandle) return;
    const a = api.getState().assembly;
    const o = orient(a, { source: state.fromNode?.id ?? null, sourceHandle: state.fromHandle.id ?? null, target: state.toNode?.id ?? null, targetHandle: state.toHandle.id ?? null } as Connection);
    const why = o && explainConnection(a, o.source, o.target);
    if (why) api.getState().toast("warning", `${why.message}${why.hint ? ` ${why.hint}` : ""}`);
  }, [api]);

  const onDragOver = useCallback((e: React.DragEvent) => { if (e.dataTransfer.types.includes(CATALOG_MIME)) { e.preventDefault(); e.dataTransfer.dropEffect = "copy"; } }, []);
  const onDrop = useCallback((e: React.DragEvent) => {
    const classId = e.dataTransfer.getData(CATALOG_MIME);
    if (!classId) return;
    e.preventDefault();
    const p = screenToFlowPosition({ x: e.clientX, y: e.clientY });
    const size = DENSITY_SIZE[api.getState().assembly.editor.density];
    api.getState().addFromCatalog(classId, { x: p.x - size.width / 2, y: p.y - size.height / 2 });
  }, [api, screenToFlowPosition]);

  const zoom = useRFStore((s) => s.transform[2]);

  return (
    <section ref={frameRef} data-labels={showLabels ? "on" : "off"} data-tool={tool} data-cable={cableStyle} data-category={showCategory ? "on" : "off"} className="apl-canvas-frame flex min-h-0 min-w-0 flex-1 flex-col" aria-label="Assembly canvas" data-testid="canvas-frame">
      <CanvasStrip zoom={zoom} />
      <div className="relative min-h-0 flex-1" data-testid="canvas" onDragOver={onDragOver} onDrop={onDrop}>
        <ReactFlow<AnyFlowNode, CableFlowEdge>
          nodes={nodes}
          edges={edges}
          nodeTypes={nodeTypes}
          edgeTypes={edgeTypes}
          edgesReconnectable
          reconnectRadius={14}
          onReconnect={onReconnect}
          onNodesChange={onNodesChange}
          onEdgesChange={onEdgesChange}
          onConnect={onConnect}
          onConnectStart={onConnectStart}
          onConnectEnd={onConnectEnd}
          isValidConnection={isValidConnection}
          onNodeDragStart={(_e, n) => { api.getState().beginGesture(); if (n.type === "shape") api.getState().beginCarry(n.id); }}
          onNodeDragStop={() => { api.getState().endGesture(); api.getState().endCarry(); api.getState().setGuides({ x: [], y: [] }); }}
          onDelete={({ nodes: n, edges: e }) => api.getState().deleteElements(n.map((x) => x.id), e.map((x) => x.id))}
          onPaneClick={() => { api.getState().closeMenu(); api.getState().setSelection({ nodes: [], edges: [], shapes: [] }); }}
          onNodeContextMenu={(e, n) => { e.preventDefault(); api.getState().openMenu({ x: e.clientX, y: e.clientY, target: n.type === "shape" ? { kind: "shape", id: n.id } : { kind: "node", id: n.id } }); }}
          onEdgeContextMenu={(e, ed) => { e.preventDefault(); api.getState().openMenu({ x: e.clientX, y: e.clientY, target: { kind: "edge", id: ed.id } }); }}
          onPaneContextMenu={(e) => { e.preventDefault(); api.getState().openMenu({ x: e.clientX, y: e.clientY, target: { kind: "pane", flow: screenToFlowPosition({ x: e.clientX, y: e.clientY }) } }); }}
          connectionMode={ConnectionMode.Loose}
          deleteKeyCode={["Backspace", "Delete"]}
          elevateNodesOnSelect={false}
          disableKeyboardA11y
          selectionOnDrag={tool === "select"}
          panOnDrag={tool === "pan" ? true : [1]}
          selectionMode={SelectionMode.Partial}
          multiSelectionKeyCode={["Control", "Meta", "Shift"]}
          panActivationKeyCode="Space"
          onNodeDoubleClick={(_e, n) => api.getState().openProps(n.type === "shape" ? { kind: "shape", id: n.id, ...(n.data && "shape" in n.data && n.data.shape.kind === "note" ? { tab: "label" as const } : {}) } : { kind: "module", id: n.id })}
          onEdgeDoubleClick={(_e, ed) => api.getState().openProps({ kind: "cable", id: ed.id })}
          snapToGrid={snap}
          snapGrid={[8, 8]}
          minZoom={0.2}
          maxZoom={2.5}
          onlyRenderVisibleElements={assembly.spec.modules.length > 150}
          proOptions={{ hideAttribution: true }}
        >
          {/* Minor lines every 16 units (2 × the snap step), major lines every 128 (one compact node width). */}
          {showGrid && <Background id="grid-minor" className="apl-grid-minor" variant={BackgroundVariant.Lines} gap={16} lineWidth={1} />}
          {showGrid && <Background id="grid-major" className="apl-grid-major" variant={BackgroundVariant.Lines} gap={128} lineWidth={1} />}
          {minimap && <MiniMap pannable zoomable ariaLabel="Overview map" nodeStrokeWidth={2} nodeColor={(n) => (n.type === "shape" ? "transparent" : "var(--color-base-content)")} maskColor="color-mix(in oklab, var(--color-base-content) 8%, transparent)" style={{ width: 150, height: 100, background: "var(--color-base-100)", bottom: 44 }} />}
          {(guides.x.length > 0 || guides.y.length > 0) && (
            <ViewportPortal>
              <svg style={{ position: "absolute", left: 0, top: 0, overflow: "visible", pointerEvents: "none" }} width="1" height="1" data-testid="guides" aria-hidden="true">
                {guides.x.map((x) => <line key={`x${x}`} className="apl-guide" x1={x} x2={x} y1={-20000} y2={20000} />)}
                {guides.y.map((y) => <line key={`y${y}`} className="apl-guide" y1={y} y2={y} x1={-20000} x2={20000} />)}
              </svg>
            </ViewportPortal>
          )}
        </ReactFlow>
        {assembly.spec.modules.length === 0 && (
          <div className="pointer-events-none absolute inset-0 flex items-center justify-center text-sm text-base-content/50">Drag a module from the catalog onto the canvas</div>
        )}
        <ValidationPanel diagnostics={diagnostics} />
        <ContextMenu />
      </div>
    </section>
  );
}
