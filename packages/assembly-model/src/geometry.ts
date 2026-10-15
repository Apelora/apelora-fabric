import type { Density, EditorNode, Port } from "./schema";

/** Geometry of an appliance node in React Flow units (not screen pixels). */
export const NODE = { width: 128, height: 96, header: 24, logo: 30, portPitch: 18, portMarker: 9, portHit: 18 } as const;

export const DENSITY_SIZE: Record<Density, { width: number; height: number }> = {
  compact: { width: 128, height: 96 },
  comfortable: { width: 152, height: 112 },
  expanded: { width: 178, height: 150 },
};

export const MIN_WIDTH = NODE.width;

/** Minimum height for a node that has to show the given number of ports on its busiest side. */
export function minApplianceHeight(inputCount: number, outputCount: number): number {
  const rows = Math.max(1, inputCount, outputCount);
  return Math.max(NODE.height, 60 + NODE.portPitch * rows);
}

export function countPorts(ports: readonly Port[]): { inputs: number; outputs: number } {
  let inputs = 0;
  for (const p of ports) if (p.direction === "input") inputs++;
  return { inputs, outputs: ports.length - inputs };
}

export function minHeightForPorts(ports: readonly Port[]): number {
  const { inputs, outputs } = countPorts(ports);
  return minApplianceHeight(inputs, outputs);
}

/** Vertical centre of the i-th port on a side, measured from the top of the node. */
export function portCenterY(index: number): number {
  return NODE.header + 18 + NODE.portPitch * index;
}

/**
 * Size actually rendered. Custom (manually resized) nodes keep their own size; the others follow
 * the density preset. In both cases the height never drops below what the ports require.
 */
export function effectiveSize(
  node: Pick<EditorNode, "width" | "height" | "custom">,
  ports: readonly Port[],
  density: Density,
): { width: number; height: number } {
  const preset = DENSITY_SIZE[density];
  const base = node.custom ? node : preset;
  return {
    width: Math.max(MIN_WIDTH, base.width),
    height: Math.max(minHeightForPorts(ports), base.height),
  };
}
