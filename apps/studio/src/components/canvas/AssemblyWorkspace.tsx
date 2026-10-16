import { useCallback, useEffect, useMemo, useRef } from "react";
import {
  Background, BackgroundVariant, ConnectionMode, MarkerType, ReactFlow, useReactFlow, useStore as useRFStore,
  type Connection, type Edge, type NodeChange, type EdgeChange, type OnConnectEnd,
} from "@xyflow/react";
import { logoUrl, lookupClass } from "@apelora/appliance-catalog";
import { DENSITY_SIZE, effectiveSize, explainConnection, minHeightForPorts, countPorts, type ApplicationAssembly, type Endpoint } from "@apelora/assembly-model";
import { useStudio, useStudioApi } from "../../features/editor/context";
import { diagnosticsFor } from "../../features/editor/store";
import { canvasBridge, CATALOG_MIME } from "../../features/editor/bridge";
import { useCustomCatalog } from "../../features/catalog/customCatalog";
import { ApplianceNode, type ApplianceFlowNode } from "./ApplianceNode";
import { CanvasStrip } from "./CanvasStrip";
import { ValidationPanel } from "../validation/ValidationPanel";

const nodeTypes = { appliance: ApplianceNode };

/** Link label = the protocol carried by the source terminal. */
function linkLabel(a: ApplicationAssembly, s: Endpoint): string {
  return a.spec.appliances.find((x) => x.id === s.applianceId)?.ports.find((p) => p.id === s.portId)?.type ?? "";
}

/** Lets users drag from either side: orient the pair output → input before validating. */
function orient(a: ApplicationAssembly, c: Pick<Connection, "source" | "target" | "sourceHandle" | "targetHandle">): { source: Endpoint; target: Endpoint } | null {
  if (!c.source || !c.target || !c.sourceHandle || !c.targetHandle) return null;
  const s: Endpoint = { applianceId: c.source, portId: c.sourceHandle };
  const t: Endpoint = { applianceId: c.target, portId: c.targetHandle };
  const sp = a.spec.appliances.find((x) => x.id === s.applianceId)?.ports.find((p) => p.id === s.portId);
  const tp = a.spec.appliances.find((x) => x.id === t.applianceId)?.ports.find((p) => p.id === t.portId);
  if (sp?.direction === "input" && tp?.direction === "output") return { source: t, target: s };
  return { source: s, target: t };
}

export function AssemblyWorkspace() {
  const api = useStudioApi();
  const assembly = useStudio((s) => s.assembly);
  const selection = useStudio((s) => s.selection);
  const showGrid = useStudio((s) => s.showGrid);
  const showLabels = useStudio((s) => s.showLabels);
  const customClasses = useCustomCatalog((s) => s.classes);
  const snap = useStudio((s) => s.snap);
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
    if (fitNonce > 0) { const t = setTimeout(() => void fitView({ padding: 0.2, maxZoom: 1.2, duration: 250 }), 60); return () => clearTimeout(t); }
  }, [fitNonce, fitView]);

  const diagnostics = useMemo(() => diagnosticsFor(assembly), [assembly]);

  const nodes = useMemo<ApplianceFlowNode[]>(() => {
    const layout = new Map(assembly.editor.nodes.map((n) => [n.id, n]));
    return assembly.spec.appliances.flatMap((a) => {
      const l = layout.get(a.id);
      if (!l) return [];
      const size = effectiveSize(l, a.ports, assembly.editor.density);
      const mine = diagnostics.filter((d) => d.applianceId === a.id);
      const hard = mine.some((d) => d.severity === "error" && d.code !== "required-port-unconnected");
      const status = hard ? "error" : mine.length ? "warning" : "ok";
      const cls = lookupClass(a.classRef.name, a.classRef.version);
      const { inputs: ni, outputs: no } = countPorts(a.ports);
      void ni; void no;
      return [{
        id: a.id,
        type: "appliance" as const,
        position: { x: l.x, y: l.y },
        width: size.width,
        measured: { width: size.width, height: size.height },
        height: size.height,
        selected: selection.nodes.includes(a.id),
        data: {
          name: a.name,
          className: cls?.displayName ?? a.classRef.name,
          icon: cls ? logoUrl(cls.iconAssetPath) : undefined,
          inputs: a.ports.filter((p) => p.direction === "input"),
          outputs: a.ports.filter((p) => p.direction === "output"),
          width: size.width,
          height: size.height,
          minHeight: minHeightForPorts(a.ports),
          status,
        },
      }];
    });
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [assembly, diagnostics, selection.nodes, customClasses]);

  const edges = useMemo<Edge[]>(() => {
    const bad = new Set(diagnostics.filter((d) => d.connectionId).map((d) => d.connectionId));
    return assembly.spec.connections.map((c) => ({
      id: c.id,
      source: c.source.applianceId,
      sourceHandle: c.source.portId,
      target: c.target.applianceId,
      targetHandle: c.target.portId,
      selected: selection.edges.includes(c.id),
      interactionWidth: 18,
      markerEnd: { type: MarkerType.ArrowClosed, width: 14, height: 14 },
      ...(showLabels ? { label: linkLabel(assembly, c.source), labelStyle: { fontSize: 9, fill: "var(--color-base-content)" }, labelBgStyle: { fill: "var(--apl-canvas)", fillOpacity: 0.95 }, labelBgPadding: [4, 2] as [number, number], labelBgBorderRadius: 4 } : {}),
      ...(bad.has(c.id) ? { "data-invalid": "true" } : {}),
      data: { invalid: bad.has(c.id) },
      className: bad.has(c.id) ? "invalid" : undefined,
    }));
  }, [assembly, diagnostics, selection.edges, showLabels]);

  const onNodesChange = useCallback((changes: NodeChange<ApplianceFlowNode>[]) => {
    const s = api.getState();
    const moves = new Map<string, { x: number; y: number }>();
    let selNodes = s.selection.nodes;
    let selChanged = false;
    for (const ch of changes) {
      if (ch.type === "position" && ch.position) moves.set(ch.id, ch.position);
      else if (ch.type === "dimensions" && ch.dimensions && (ch.resizing || ch.setAttributes)) {
        s.resizeTransient(ch.id, ch.dimensions);
      } else if (ch.type === "select") {
        selChanged = true;
        selNodes = ch.selected ? [...new Set([...selNodes, ch.id])] : selNodes.filter((x) => x !== ch.id);
      }
    }
    if (moves.size) s.moveTransient(moves);
    if (selChanged) s.setSelection({ nodes: selNodes });
  }, [api]);

  const onEdgesChange = useCallback((changes: EdgeChange[]) => {
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

  const onConnect = useCallback((c: Connection) => {
    const o = orient(api.getState().assembly, c);
    if (o) api.getState().connectPorts(o.source, o.target);
  }, [api]);

  const onConnectEnd = useCallback<OnConnectEnd>((_e, state) => {
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
    <section ref={frameRef} data-labels={showLabels ? "on" : "off"} className="apl-canvas-frame flex min-h-0 min-w-0 flex-1 flex-col" aria-label="Assembly canvas" data-testid="canvas-frame">
      <CanvasStrip zoom={zoom} />
      <div className="relative min-h-0 flex-1" data-testid="canvas" onDragOver={onDragOver} onDrop={onDrop}>
        <ReactFlow<ApplianceFlowNode>
          nodes={nodes}
          edges={edges}
          nodeTypes={nodeTypes}
          onNodesChange={onNodesChange}
          onEdgesChange={onEdgesChange}
          onConnect={onConnect}
          onConnectEnd={onConnectEnd}
          isValidConnection={isValidConnection}
          onNodeDragStart={() => api.getState().beginGesture()}
          onNodeDragStop={() => api.getState().endGesture()}
          onDelete={({ nodes: n, edges: e }) => api.getState().deleteElements(n.map((x) => x.id), e.map((x) => x.id))}
          onPaneClick={() => api.getState().setSelection({ nodes: [], edges: [] })}
          connectionMode={ConnectionMode.Loose}
          deleteKeyCode={["Backspace", "Delete"]}
          snapToGrid={snap}
          snapGrid={[8, 8]}
          minZoom={0.2}
          maxZoom={2.5}
          proOptions={{ hideAttribution: true }}
          defaultEdgeOptions={{ type: "default" }}
        >
          {showGrid && <Background variant={BackgroundVariant.Dots} gap={20} size={1} />}
        </ReactFlow>
        {assembly.spec.appliances.length === 0 && (
          <div className="pointer-events-none absolute inset-0 flex items-center justify-center text-sm text-base-content/50">Drag an appliance from the catalog onto the canvas</div>
        )}
        <ValidationPanel diagnostics={diagnostics} />
      </div>
    </section>
  );
}
