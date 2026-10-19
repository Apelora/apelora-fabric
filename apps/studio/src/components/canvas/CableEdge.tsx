import { BaseEdge, EdgeLabelRenderer, getBezierPath, getSmoothStepPath, type Edge, type EdgeProps } from "@xyflow/react";
import { CABLE_INSET, cableColor, cableOutline } from "./cables";

export type CableData = { type: string; label?: string; color?: string; invalid?: boolean; angled?: boolean; [key: string]: unknown };
export type CableFlowEdge = Edge<CableData, "cable">;

/** A patch cable: coloured by protocol unless the user picked a colour, with a flow arrow near its target plug. The plug heads live in the nodes. */
export function CableEdge({ id, sourceX, sourceY, targetX, targetY, sourcePosition, targetPosition, data }: EdgeProps<CableFlowEdge>) {
  const sx = sourceX + CABLE_INSET;
  const tx = targetX - CABLE_INSET;
  const [path, lx, ly] = data?.angled
    ? getSmoothStepPath({ sourceX: sx, sourceY, sourcePosition, targetX: tx, targetY, targetPosition, borderRadius: 6, offset: 14 })
    : getBezierPath({ sourceX: sx, sourceY, sourcePosition, targetX: tx, targetY, targetPosition });
  const type = data?.type ?? "tcp";
  // An invalid cable is always shown in the error colour, even when the user chose a colour.
  const base = data?.color ?? cableColor(type);
  const color = data?.invalid ? "var(--color-error)" : base;
  return (
    <>
      <path d={path} fill="none" className="apl-cable-outline" style={{ stroke: data?.invalid ? "var(--color-error)" : cableOutline(base) }} />
      <BaseEdge id={id} path={path} pathLength={1} interactionWidth={18} className={`apl-cable ${data?.invalid ? "apl-cable-invalid" : ""}`} style={{ stroke: color }} />
      <path d={`M ${tx - 9} ${targetY - 2.6} L ${tx - 2.5} ${targetY} L ${tx - 9} ${targetY + 2.6} Z`} fill={color} className="apl-cable-arrow" />
      {data?.label && (
        <EdgeLabelRenderer>
          <div className="apl-link-label nodrag nopan" data-testid="link-label" style={{ transform: `translate(-50%, -50%) translate(${lx}px, ${ly}px)` }}>{data.label}</div>
        </EdgeLabelRenderer>
      )}
    </>
  );
}
