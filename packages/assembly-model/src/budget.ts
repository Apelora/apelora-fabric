import { replicasOf } from "./runtime";
import type { ApplicationAssembly, ModuleClass, ResourceBudget } from "./schema";

type ClassLookup = (name: string, version: string) => ModuleClass | undefined;

export interface BudgetProblem {
  message: string;
  hint: string;
}

const KEYS = ["cpuMin", "cpuMax", "memoryMinMiB", "memoryMaxMiB"] as const;

/** vCPU and memory the application asks for, once per replica. */
export function applicationDemand(a: ApplicationAssembly, lookup: ClassLookup): { cpu: number; memoryMiB: number } {
  let cpu = 0;
  let memoryMiB = 0;
  for (const m of a.spec.modules) {
    const n = replicasOf(lookup(m.classRef.name, m.classRef.version), m);
    cpu += m.resources.cpu * n;
    memoryMiB += m.resources.memoryMiB * n;
  }
  return { cpu, memoryMiB };
}

/** The design is outside its own budget, or a minimum is above its maximum. */
export function budgetIssues(a: ApplicationAssembly, lookup: ClassLookup): BudgetProblem[] {
  const b = a.spec.budget;
  if (!b) return [];
  const out: BudgetProblem[] = [];
  if (b.cpuMin !== undefined && b.cpuMax !== undefined && b.cpuMin > b.cpuMax) {
    out.push({ message: "The vCPU minimum is above the maximum.", hint: "Raise the maximum or lower the minimum." });
  }
  if (b.memoryMinMiB !== undefined && b.memoryMaxMiB !== undefined && b.memoryMinMiB > b.memoryMaxMiB) {
    out.push({ message: "The memory minimum is above the maximum.", hint: "Raise the maximum or lower the minimum." });
  }
  const asked = applicationDemand(a, lookup);
  if (b.cpuMin !== undefined && asked.cpu < b.cpuMin) {
    out.push({ message: `The application asks for ${asked.cpu} vCPU, below the minimum of ${b.cpuMin}.`, hint: "Raise a module's vCPU or lower the minimum. Replicas are counted." });
  }
  if (b.cpuMax !== undefined && asked.cpu > b.cpuMax) {
    out.push({ message: `The application asks for ${asked.cpu} vCPU, above the maximum of ${b.cpuMax}.`, hint: "Lower a module's vCPU or raise the maximum. Replicas are counted." });
  }
  if (b.memoryMinMiB !== undefined && asked.memoryMiB < b.memoryMinMiB) {
    out.push({ message: `The application asks for ${asked.memoryMiB} MiB, below the minimum of ${b.memoryMinMiB} MiB.`, hint: "Raise a module's memory or lower the minimum. Replicas are counted." });
  }
  if (b.memoryMaxMiB !== undefined && asked.memoryMiB > b.memoryMaxMiB) {
    out.push({ message: `The application asks for ${asked.memoryMiB} MiB, above the maximum of ${b.memoryMaxMiB} MiB.`, hint: "Lower a module's memory or raise the maximum. Replicas are counted." });
  }
  return out;
}

/** Sets one bound. An empty value removes it. No bounds at all removes the budget from the file. */
export function setBudget(a: ApplicationAssembly, patch: Partial<Record<keyof ResourceBudget, number | undefined>>): ApplicationAssembly {
  const merged: Partial<Record<keyof ResourceBudget, number | undefined>> = { ...a.spec.budget, ...patch };
  const budget: ResourceBudget = {};
  for (const key of KEYS) {
    const v = merged[key];
    if (typeof v === "number" && Number.isFinite(v) && v > 0) budget[key] = v;
  }
  const { budget: _drop, ...spec } = a.spec;
  void _drop;
  return { ...a, spec: Object.keys(budget).length > 0 ? { ...spec, budget } : spec };
}
