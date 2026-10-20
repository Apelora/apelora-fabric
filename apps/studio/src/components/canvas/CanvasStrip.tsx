import { useReactFlow } from "@xyflow/react";
import { useStudio, useStudioApi } from "../../features/editor/context";
import { canvasBridge } from "../../features/editor/bridge";
import { IconCircle, IconLayout, IconMap, IconNote, IconRounded, IconSquare, IconTriangle, IconCursor, IconFit, IconFocus, IconGrid, IconHand, IconMagnet, IconMinus, IconPlus } from "../icons";
import { ms } from "../../features/editor/motion";

export function CanvasStrip({ zoom }: { zoom: number }) {
  const api = useStudioApi();
  const name = useStudio((s) => s.assembly.metadata.name);
  const showGrid = useStudio((s) => s.showGrid);
  const snap = useStudio((s) => s.snap);
  const showLabels = useStudio((s) => s.showLabels);
  const cableStyle = useStudio((s) => s.cableStyle);
  const routing = useStudio((s) => s.routing);
  const tool = useStudio((s) => s.tool);
  const minimap = useStudio((s) => s.minimap);
  // Count appliances and zones; cables (which the selection box also picks up) only count when nothing else is selected.
  const nSel = useStudio((s) => { const n = s.selection.nodes.length + (s.selection.shapes?.length ?? 0); return n > 0 ? n : s.selection.edges.length; });
  const selected = useStudio((s) => s.selection.nodes);
  const { zoomIn, zoomOut, fitView, screenToFlowPosition } = useReactFlow();

  return (
    <div className="apl-canvas-strip flex h-9 shrink-0 items-center gap-3 px-3">
      <div className="flex items-center gap-1.5" aria-hidden="true">
        <span className="size-2.5 rounded-full bg-base-content/15" /><span className="size-2.5 rounded-full bg-base-content/15" /><span className="size-2.5 rounded-full bg-base-content/15" />
      </div>
      <div className="join" role="group" aria-label="Tool">
        <button className={`btn btn-xs join-item ${tool === "select" ? "btn-active" : ""}`} aria-pressed={tool === "select"} title="Select tool (V): click, or drag a box to select several. Hold Space to pan." aria-label="Select tool" data-testid="tool-select" onClick={() => api.getState().setTool("select")}><IconCursor size={14} /></button>
        <button className={`btn btn-xs join-item ${tool === "pan" ? "btn-active" : ""}`} aria-pressed={tool === "pan"} title="Pan tool (H): drag to move the view" aria-label="Pan tool" data-testid="tool-pan" onClick={() => api.getState().setTool("pan")}><IconHand size={14} /></button>
      </div>
      <input
        key={name}
        defaultValue={name}
        aria-label="Assembly name"
        data-testid="assembly-name"
        className="input input-ghost input-xs h-6 w-48 min-w-0 flex-1 bg-transparent text-xs font-medium focus:bg-base-100 sm:flex-none"
        onBlur={(e) => api.getState().renameAssembly(e.currentTarget.value)}
        onKeyDown={(e) => { if (e.key === "Enter") e.currentTarget.blur(); }}
      />
      {nSel > 0 && <span className="hidden text-[11px] text-base-content/75 md:inline" data-testid="selection-info" aria-live="polite">{nSel} selected</span>}
      <div className="dropdown" data-testid="shapes-dropdown">
        <button tabIndex={0} className="btn btn-xs" aria-label="Add zone or shape" title="Add a zone: a labelled shape behind appliances" data-testid="shapes-button"><IconSquare size={14} /><span className="hidden sm:inline">Zones</span></button>
        <ul tabIndex={0} className="menu dropdown-content z-30 mt-1 w-44 rounded-box border border-base-300 bg-base-100 p-1 text-xs shadow-lg">
          {([["rect", "Square", IconSquare], ["ellipse", "Circle", IconCircle], ["rounded", "Rounded square", IconRounded], ["triangle", "Triangle", IconTriangle], ["note", "Sticky note", IconNote]] as const).map(([k, label, I]) => (
            <li key={k}><button type="button" data-testid={`add-shape-${k}`} onClick={() => { const el = canvasBridge.el; const r = el?.getBoundingClientRect(); const c = r ? screenToFlowPosition({ x: r.left + r.width / 2, y: r.top + r.height / 2 }) : undefined; api.getState().addShapeAt(k, c, true); (document.activeElement as HTMLElement | null)?.blur(); }}><I size={14} />{label}</button></li>
          ))}
          <li className="menu-title px-2 pt-1 text-[10px]" role="presentation">Select appliances first, then right-click → Wrap in zone</li>
        </ul>
      </div>
      <button type="button" className="btn btn-xs" title="Tidy layout: arrange left to right along the cables (Shift+T)" aria-label="Tidy layout" data-testid="tidy" onClick={() => api.getState().tidy()}><IconLayout size={14} /><span className="hidden lg:inline">Tidy</span></button>
      <div className="ml-auto flex items-center gap-1.5">
        <div className="join" role="group" aria-label="Canvas options">
          <button className={`btn btn-xs join-item ${showGrid ? "btn-active" : ""}`} aria-pressed={showGrid} title="Toggle grid" aria-label="Toggle grid" onClick={() => api.getState().setShowGrid(!showGrid)}><IconGrid size={14} /></button>
          <button className={`btn btn-xs join-item ${showLabels ? "btn-active" : ""}`} aria-pressed={showLabels} title="Show port and link labels" aria-label="Port and link labels" data-testid="toggle-labels" onClick={() => api.getState().setShowLabels(!showLabels)}><span className="text-[10px] font-semibold leading-none">Aa</span></button>
          <button className={`btn btn-xs join-item ${cableStyle === "cable" ? "btn-active" : ""}`} aria-pressed={cableStyle === "cable"} title={cableStyle === "line" ? "Switch to thick patch-cable look" : "Switch to thin line look"} aria-label="Cable style" data-testid="toggle-cable-style" onClick={() => api.getState().setCableStyle(cableStyle === "line" ? "cable" : "line")}><svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={cableStyle === "cable" ? 4 : 1.6} strokeLinecap="round" aria-hidden="true"><path d="M3 18C9 18 9 6 15 6h6" /></svg></button>
          <button className={`btn btn-xs join-item ${routing === "angled" ? "btn-active" : ""}`} aria-pressed={routing === "angled"} title={routing === "angled" ? "Switch to curved cables" : "Switch to right-angle cables"} aria-label="Cable routing" data-testid="toggle-routing" onClick={() => api.getState().setRouting(routing === "angled" ? "curved" : "angled")}><svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d={routing === "angled" ? "M3 18H12V6H21" : "M3 18C9 18 9 6 15 6H21"} /></svg></button>
          <button className={`btn btn-xs join-item ${minimap ? "btn-active" : ""}`} aria-pressed={minimap} title="Overview map" aria-label="Overview map" data-testid="toggle-minimap" onClick={() => api.getState().setMinimap(!minimap)}><IconMap size={14} /></button>
          <button className={`btn btn-xs join-item ${snap ? "btn-active" : ""}`} aria-pressed={snap} title="Snap to grid" aria-label="Snap to grid" onClick={() => api.getState().setSnap(!snap)}><IconMagnet size={14} /></button>
        </div>
        <div className="join" role="group" aria-label="Zoom">
          <button className="btn btn-xs join-item" title="Zoom out (−)" aria-label="Zoom out" onClick={() => void zoomOut({ duration: ms(150) })}><IconMinus size={14} /></button>
          <span className="btn btn-xs join-item pointer-events-none w-12 font-normal tabular-nums" data-testid="zoom-level" aria-live="polite">{Math.round(zoom * 100)}%</span>
          <button className="btn btn-xs join-item" title="Zoom in (+)" aria-label="Zoom in" onClick={() => void zoomIn({ duration: ms(150) })}><IconPlus size={14} /></button>
          <button className="btn btn-xs join-item" title="Fit to view (F)" aria-label="Fit to view" onClick={() => void fitView({ padding: 0.2, maxZoom: 1.2, duration: ms(250) })}><IconFit size={14} /></button>
          <button className="btn btn-xs join-item" title="Zoom to selection" aria-label="Zoom to selection" disabled={selected.length === 0} onClick={() => void fitView({ nodes: selected.map((id) => ({ id })), padding: 0.6, maxZoom: 1.5, duration: ms(250) })}><IconFocus size={14} /></button>
        </div>
      </div>
    </div>
  );
}
