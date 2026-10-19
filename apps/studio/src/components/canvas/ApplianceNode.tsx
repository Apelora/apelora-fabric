import { memo, useEffect } from "react";
import { Handle, NodeResizeControl, Position, useUpdateNodeInternals, type Node, type NodeProps } from "@xyflow/react";
import { NODE, portCenterY, type Category, type Port } from "@apelora/assembly-model";
import { categoryColor } from "../../features/editor/categories";
import { useStudioApi } from "../../features/editor/context";
import { Jack, PlugHead } from "./Jack";

export type ApplianceNodeData = {
  name: string;
  className: string;
  category: Category;
  icon: string | undefined;
  inputs: Port[];
  outputs: Port[];
  width: number;
  height: number;
  minHeight: number;
  status: "ok" | "warning" | "error";
  /** Cables plugged into each port: count and the protocol colour of the first cable. */
  links: Record<string, { count: number; type: string }>;
  [key: string]: unknown;
};
export type ApplianceFlowNode = Node<ApplianceNodeData, "appliance">;

function PortHandle({ nodeId, port, index, side, link }: { nodeId: string; port: Port; index: number; side: "input" | "output"; link: { count: number; type: string } | undefined }) {
  const api = useStudioApi();
  const label = `${port.name} (${port.type})${port.optional ? "" : ", required"}, ${port.multiplicity === "one" ? "single cable" : "multiple cables"}${link ? `, ${link.count} plugged` : ""}${port.description ? ` — ${port.description}` : ""}`;
  return (
    <Handle
      id={port.id}
      type={side === "input" ? "target" : "source"}
      position={side === "input" ? Position.Left : Position.Right}
      className="apl-handle"
      data-side={side}
      data-kind={side}
      data-port-id={port.id}
      style={{ top: portCenterY(index) }}
      title={label}
      aria-label={label}
      onContextMenu={(e) => { e.preventDefault(); e.stopPropagation(); api.getState().openMenu({ x: e.clientX, y: e.clientY, target: { kind: "port", nodeId, portId: port.id } }); }}
    >
      <Jack kind={side} />
      {link && <PlugHead side={side} type={link.type} count={link.count} />}
      <span className="apl-port-label" data-testid="port-label" aria-hidden="true">{port.name}</span>
      <span className="apl-port-tip" aria-hidden="true">{label}</span>
    </Handle>
  );
}

function ApplianceNodeImpl({ id, data, selected }: NodeProps<ApplianceFlowNode>) {
  const api = useStudioApi();
  const updateInternals = useUpdateNodeInternals();
  const signature = `${data.inputs.map((p) => p.id).join(",")}|${data.outputs.map((p) => p.id).join(",")}|${data.width}x${data.height}`;
  useEffect(() => { updateInternals(id); }, [id, signature, updateInternals]);

  return (
    <div
      className="apl-node"
      data-testid="appliance-node"
      data-node-id={id}
      data-selected={selected ? "true" : "false"}
      data-status={data.status}
      style={{ width: data.width, height: data.height, ["--apl-node-w" as string]: `${data.width}px`, ["--apl-cat" as string]: categoryColor(data.category) }}
    >
      <div className="apl-node-header flex items-center gap-1 px-2 text-[11px] font-medium leading-none" title={`${data.name} · ${data.className}`}>
        <span className="min-w-0 flex-1 truncate">{data.name}</span>
        {data.status !== "ok" && <span className={`size-1.5 shrink-0 rounded-full ${data.status === "error" ? "bg-error" : "bg-warning"}`} aria-label={data.status} />}
      </div>
      <div className="relative flex items-center justify-center" style={{ height: data.height - NODE.header - 2 }}>
        {data.icon ? <img src={data.icon} alt="" width={NODE.logo} height={NODE.logo} style={{ width: NODE.logo, height: NODE.logo }} draggable={false} data-testid="node-logo" /> : <div className="rounded bg-base-300" style={{ width: NODE.logo, height: NODE.logo }} />}
        <span className="apl-node-class" data-testid="node-class">{data.className}</span>
      </div>
      {data.inputs.map((p, i) => <PortHandle key={p.id} nodeId={id} port={p} index={i} side="input" link={data.links[p.id]} />)}
      {data.outputs.map((p, i) => <PortHandle key={p.id} nodeId={id} port={p} index={i} side="output" link={data.links[p.id]} />)}
      <NodeResizeControl
        className="apl-resize-control"
        position="bottom-right"
        minWidth={NODE.width}
        minHeight={data.minHeight}
        onResizeStart={() => api.getState().beginGesture()}
        onResizeEnd={() => api.getState().endGesture()}
        aria-label="Resize appliance"
      >
        <svg className="apl-resize-grip" viewBox="0 0 10 10" aria-hidden="true"><path d="M9 1L1 9M9 5L5 9" stroke="currentColor" strokeWidth="1.2" strokeLinecap="round" fill="none" /></svg>
      </NodeResizeControl>
    </div>
  );
}

export const ApplianceNode = memo(ApplianceNodeImpl);
