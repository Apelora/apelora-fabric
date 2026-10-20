import { memo } from "react";
import { NodeResizer, type Node, type NodeProps } from "@xyflow/react";
import type { Shape } from "@apelora/assembly-model";
import { useStudioApi } from "../../features/editor/context";

export type ShapeNodeData = { shape: Shape; inside: number; [key: string]: unknown };
export type ShapeFlowNode = Node<ShapeNodeData, "shape">;

function dash(style: Shape["strokeStyle"], w: number): string | undefined {
  if (style === "dashed") return `${Math.max(6, w * 4)} ${Math.max(4, w * 3)}`;
  if (style === "dotted") return `${Math.max(1, w)} ${Math.max(4, w * 2.5)}`;
  return undefined;
}

/** A translucent zone/sector behind modules. Only its border and label take pointer events, so cables and modules under it stay usable. */
function ShapeNodeImpl({ id, data, selected }: NodeProps<ShapeFlowNode>) {
  const api = useStudioApi();
  const s = data.shape;
  const w = s.width;
  const h = s.height;
  const sw = s.strokeWidth;
  const half = sw / 2;
  const common = { fill: s.fill, fillOpacity: s.fillOpacity, stroke: sw > 0 ? s.stroke : "none", strokeWidth: sw, strokeDasharray: dash(s.strokeStyle, sw), strokeLinecap: "round" as const, strokeLinejoin: "round" as const };
  const hit = { fill: "none", stroke: "transparent", strokeWidth: Math.max(sw, 14), className: "apl-shape-hit" };
  const r = Math.min(s.radius, w / 2, h / 2);
  const tri = `${w / 2},${half} ${w - half},${h - half} ${half},${h - half}`;

  if (s.kind === "note") {
    return (
      <div className="apl-shape apl-note" data-testid="shape-node" data-shape-id={id} data-kind="note" data-selected={selected ? "true" : "false"} data-locked={s.locked}
        style={{ width: w, height: h, background: s.fill, borderColor: s.stroke, fontSize: s.fontSize, opacity: 1 }}>
        <div className="apl-note-text" data-testid="note-body">{s.text ?? ""}</div>
        {s.locked && <span aria-label="Locked" title="Locked" className="apl-note-lock">🔒</span>}
        <NodeResizer isVisible={selected && !s.locked} minWidth={90} minHeight={50}
          lineStyle={{ borderColor: "color-mix(in oklab, var(--color-base-content) 40%, transparent)" }}
          handleStyle={{ width: 9, height: 9, borderRadius: 2, background: "var(--color-base-100)", border: "1.5px solid var(--color-base-content)" }}
          onResizeStart={() => api.getState().beginGesture()} onResizeEnd={() => api.getState().endGesture()} />
      </div>
    );
  }

  return (
    <div className="apl-shape" data-testid="shape-node" data-shape-id={id} data-kind={s.kind} data-selected={selected ? "true" : "false"} data-locked={s.locked} style={{ width: w, height: h }}>
      <svg width={w} height={h} viewBox={`0 0 ${w} ${h}`} aria-hidden="true" style={{ overflow: "visible" }}>
        {s.kind === "ellipse" && <><ellipse cx={w / 2} cy={h / 2} rx={w / 2 - half} ry={h / 2 - half} {...common} pointerEvents="none" /><ellipse cx={w / 2} cy={h / 2} rx={w / 2 - half} ry={h / 2 - half} {...hit} /></>}
        {(s.kind === "rect" || s.kind === "rounded") && <><rect x={half} y={half} width={w - sw} height={h - sw} rx={s.kind === "rounded" ? r : 0} {...common} pointerEvents="none" /><rect x={half} y={half} width={w - sw} height={h - sw} rx={s.kind === "rounded" ? r : 0} {...hit} /></>}
        {s.kind === "triangle" && <><polygon points={tri} {...common} pointerEvents="none" /><polygon points={tri} {...hit} /></>}
      </svg>
      <div className="apl-shape-label" data-pos={s.labelPos} data-kind={s.kind} style={{ fontSize: s.fontSize }} data-testid="shape-label">
        {s.locked && <span aria-label="Locked" title="Locked" className="mr-1 opacity-60">🔒</span>}{s.label}
      </div>
      <NodeResizer
        isVisible={selected && !s.locked}
        minWidth={60}
        minHeight={40}
        lineStyle={{ borderColor: "color-mix(in oklab, var(--color-base-content) 40%, transparent)" }}
        handleStyle={{ width: 9, height: 9, borderRadius: 2, background: "var(--color-base-100)", border: "1.5px solid var(--color-base-content)" }}
        onResizeStart={() => api.getState().beginGesture()}
        onResizeEnd={() => api.getState().endGesture()}
      />
    </div>
  );
}

export const ShapeNode = memo(ShapeNodeImpl);
