import { BaseEdge, EdgeLabelRenderer, getBezierPath, type Edge, type EdgeProps } from "@xyflow/react";
import { CABLE_INSET, cableColor, cableOutline } from "./cables";

export type CableData = { type: string; label?: string; invalid?: boolean; [key: string]: unknown };
export type CableFlowEdge = Edge<CableData, "cable">;

/** A patch cable: coloured by protocol, with a flow arrow near its target plug. The plug heads live in the nodes. */
export function CableEdge({ id, sourceX, sourceY, targetX, targetY, sourcePosition, targetPosition, data }: EdgeProps<CableFlowEdge>) {
  const sx = sourceX + CABLE_INSET;
  const tx = targetX - CABLE_INSET;
  const [path, lx, ly] = getBezierPath({ sourceX: sx, sourceY, sourcePosition, targetX: tx, targetY, targetPosition });
  const type = data?.type ?? "tcp";
  const color = data?.invalid ? "var(--color-error)" : cableColor(type);
  return (
    <>
      <path d={path} fill="none" className="apl-cable-outline" style={{ stroke: data?.invalid ? "var(--color-error)" : cableOutline(type) }} />
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
