import { shapeContents } from "./shapes";
import type { ApplicationAssembly } from "./schema";

export interface Totals { appliances: number; cpu: number; memoryMiB: number; cables: number }

const sum = (a: ApplicationAssembly, ids: ReadonlySet<string>): Totals => {
  let cpu = 0;
  let memoryMiB = 0;
  let appliances = 0;
  for (const x of a.spec.appliances) {
    if (!ids.has(x.id)) continue;
    appliances++;
    cpu += x.resources.cpu;
    memoryMiB += x.resources.memoryMiB;
  }
  const cables = a.spec.connections.filter((c) => ids.has(c.source.applianceId) && ids.has(c.target.applianceId)).length;
  return { appliances, cpu, memoryMiB, cables };
};

/** vCPU and memory requested by every appliance, and the number of cables. */
export function assemblyTotals(a: ApplicationAssembly): Totals {
  return sum(a, new Set(a.spec.appliances.map((x) => x.id)));
}

/** The same figures for the appliances inside a zone (cables count only when both ends are inside). */
export function zoneTotals(a: ApplicationAssembly, shapeId: string): Totals {
  return sum(a, new Set(shapeContents(a, shapeId)));
}

export function formatMemory(mib: number): string {
  if (mib >= 1024) { const g = mib / 1024; return `${Number.isInteger(g) ? g : g.toFixed(1)} GiB`; }
  return `${mib} MiB`;
}
