import { useReactFlow } from "@xyflow/react";
import { useStudio, useStudioApi } from "../../features/editor/context";
import { IconFit, IconFocus, IconGrid, IconMagnet, IconMinus, IconPlus } from "../icons";
import { ms } from "../../features/editor/motion";

export function CanvasStrip({ zoom }: { zoom: number }) {
  const api = useStudioApi();
  const name = useStudio((s) => s.assembly.metadata.name);
  const showGrid = useStudio((s) => s.showGrid);
  const snap = useStudio((s) => s.snap);
  const showLabels = useStudio((s) => s.showLabels);
  const cableStyle = useStudio((s) => s.cableStyle);
  const selected = useStudio((s) => s.selection.nodes);
  const { zoomIn, zoomOut, fitView } = useReactFlow();

  return (
    <div className="apl-canvas-strip flex h-9 shrink-0 items-center gap-3 px-3">
      <div className="flex items-center gap-1.5" aria-hidden="true">
        <span className="size-2.5 rounded-full bg-base-content/15" /><span className="size-2.5 rounded-full bg-base-content/15" /><span className="size-2.5 rounded-full bg-base-content/15" />
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
      <div className="ml-auto flex items-center gap-1.5">
        <div className="join" role="group" aria-label="Canvas options">
          <button className={`btn btn-xs join-item ${showGrid ? "btn-active" : ""}`} aria-pressed={showGrid} title="Toggle dotted grid" aria-label="Toggle dotted grid" onClick={() => api.getState().setShowGrid(!showGrid)}><IconGrid size={14} /></button>
          <button className={`btn btn-xs join-item ${showLabels ? "btn-active" : ""}`} aria-pressed={showLabels} title="Show port and link labels" aria-label="Port and link labels" data-testid="toggle-labels" onClick={() => api.getState().setShowLabels(!showLabels)}><span className="text-[10px] font-semibold leading-none">Aa</span></button>
          <button className={`btn btn-xs join-item ${cableStyle === "cable" ? "btn-active" : ""}`} aria-pressed={cableStyle === "cable"} title={cableStyle === "line" ? "Switch to thick patch-cable look" : "Switch to thin line look"} aria-label="Cable style" data-testid="toggle-cable-style" onClick={() => api.getState().setCableStyle(cableStyle === "line" ? "cable" : "line")}><svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={cableStyle === "cable" ? 4 : 1.6} strokeLinecap="round" aria-hidden="true"><path d="M3 18C9 18 9 6 15 6h6" /></svg></button>
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
