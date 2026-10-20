import { shapeContents } from "./shapes";
import type { ApplicationAssembly } from "./schema";

export interface Totals { modules: number; cpu: number; memoryMiB: number; cables: number }

const sum = (a: ApplicationAssembly, ids: ReadonlySet<string>): Totals => {
  let cpu = 0;
  let memoryMiB = 0;
  let modules = 0;
  for (const x of a.spec.modules) {
    if (!ids.has(x.id)) continue;
    modules++;
    cpu += x.resources.cpu;
    memoryMiB += x.resources.memoryMiB;
  }
  const cables = a.spec.connections.filter((c) => ids.has(c.source.moduleId) && ids.has(c.target.moduleId)).length;
  return { modules, cpu, memoryMiB, cables };
};

/** vCPU and memory requested by every module, and the number of cables. */
export function assemblyTotals(a: ApplicationAssembly): Totals {
  return sum(a, new Set(a.spec.modules.map((x) => x.id)));
}

/** The same figures for the modules inside a zone (cables count only when both ends are inside). */
export function zoneTotals(a: ApplicationAssembly, shapeId: string): Totals {
  return sum(a, new Set(shapeContents(a, shapeId)));
}

export function formatMemory(mib: number): string {
  if (mib >= 1024) { const g = mib / 1024; return `${Number.isInteger(g) ? g : g.toFixed(1)} GiB`; }
  return `${mib} MiB`;
}
