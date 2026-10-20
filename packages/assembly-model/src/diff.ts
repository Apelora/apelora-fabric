import type { ApplicationAssembly, Connection } from "./schema";

export interface AssemblyDiff {
  addedAppliances: string[];
  removedAppliances: string[];
  /** Appliances present on both sides whose settings differ, with a short description of what changed. */
  changedAppliances: { name: string; changes: string[] }[];
  addedCables: string[];
  removedCables: string[];
  movedAppliances: number;
  zonesBefore: number;
  zonesAfter: number;
  /** Nothing differs, not even layout. */
  identical: boolean;
}

const cableText = (a: ApplicationAssembly, c: Connection): string => {
  const name = (id: string) => a.spec.appliances.find((x) => x.id === id)?.name ?? id;
  const port = (id: string, pid: string) => a.spec.appliances.find((x) => x.id === id)?.ports.find((p) => p.id === pid)?.name ?? pid;
  return `${name(c.source.applianceId)}.${port(c.source.applianceId, c.source.portId)} → ${name(c.target.applianceId)}.${port(c.target.applianceId, c.target.portId)}`;
};
const cableKey = (c: Connection) => `${c.source.applianceId}.${c.source.portId}>${c.target.applianceId}.${c.target.portId}`;

/** Summarises what changed between two assemblies (a snapshot and the current diagram, for instance). */
export function diffAssemblies(before: ApplicationAssembly, after: ApplicationAssembly): AssemblyDiff {
  const b = new Map(before.spec.appliances.map((x) => [x.id, x]));
  const a = new Map(after.spec.appliances.map((x) => [x.id, x]));
  const changedAppliances: AssemblyDiff["changedAppliances"] = [];
  for (const [id, x] of a) {
    const y = b.get(id);
    if (!y) continue;
    const changes: string[] = [];
    if (x.name !== y.name) changes.push(`renamed from “${y.name}”`);
    if (x.classRef.name !== y.classRef.name || x.classRef.version !== y.classRef.version) changes.push(`class ${y.classRef.name} → ${x.classRef.name}`);
    if (x.resources.cpu !== y.resources.cpu) changes.push(`vCPU ${y.resources.cpu} → ${x.resources.cpu}`);
    if (x.resources.memoryMiB !== y.resources.memoryMiB) changes.push(`memory ${y.resources.memoryMiB} → ${x.resources.memoryMiB} MiB`);
    if (x.ports.length !== y.ports.length) changes.push(`${y.ports.length} → ${x.ports.length} ports`);
    const keys = new Set([...Object.keys(x.config ?? {}), ...Object.keys(y.config ?? {})]);
    for (const k of [...keys].sort()) if (x.config?.[k] !== y.config?.[k]) changes.push(`setting “${k}” changed`);
    if (changes.length) changedAppliances.push({ name: x.name, changes });
  }
  const bc = new Map(before.spec.connections.map((c) => [cableKey(c), c]));
  const ac = new Map(after.spec.connections.map((c) => [cableKey(c), c]));
  const bl = new Map(before.editor.nodes.map((n) => [n.id, n]));
  let moved = 0;
  for (const n of after.editor.nodes) { const o = bl.get(n.id); if (o && (o.x !== n.x || o.y !== n.y)) moved++; }
  const nameOf = (m: Map<string, ApplicationAssembly["spec"]["appliances"][number]>, id: string) => m.get(id)?.name ?? id;
  const d: AssemblyDiff = {
    addedAppliances: [...a.keys()].filter((id) => !b.has(id)).map((id) => nameOf(a, id)),
    removedAppliances: [...b.keys()].filter((id) => !a.has(id)).map((id) => nameOf(b, id)),
    changedAppliances,
    addedCables: [...ac].filter(([k]) => !bc.has(k)).map(([, c]) => cableText(after, c)),
    removedCables: [...bc].filter(([k]) => !ac.has(k)).map(([, c]) => cableText(before, c)),
    movedAppliances: moved,
    zonesBefore: before.editor.shapes?.length ?? 0,
    zonesAfter: after.editor.shapes?.length ?? 0,
    identical: false,
  };
  d.identical = !d.addedAppliances.length && !d.removedAppliances.length && !d.changedAppliances.length && !d.addedCables.length && !d.removedCables.length && !d.movedAppliances && d.zonesBefore === d.zonesAfter;
  return d;
}
