import { API_VERSION, ASSEMBLY_KIND, LAYOUT_VERSION } from "./constants";
import { DENSITY_SIZE, effectiveSize, minHeightForPorts } from "./geometry";
import { explainConnection } from "./validate";
import type { ApplianceClass, ApplicationAssembly, ApplianceInstance, Connection, Density, Endpoint, Port, PortDirection, PortType } from "./schema";

export function slug(input: string): string {
  const s = input.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "");
  return /^[a-z]/.test(s) ? s.slice(0, 40) : `x-${s}`.slice(0, 40);
}

function uniqueId(base: string, taken: ReadonlySet<string>): string {
  let n = 1;
  let id = `${base}-${String(n).padStart(2, "0")}`;
  while (taken.has(id)) id = `${base}-${String(++n).padStart(2, "0")}`;
  return id;
}

export function createEmptyAssembly(name = "untitled-assembly"): ApplicationAssembly {
  return {
    apiVersion: API_VERSION,
    kind: ASSEMBLY_KIND,
    metadata: { name, uid: `assembly-${Math.random().toString(36).slice(2, 10)}` },
    spec: { appliances: [], connections: [] },
    editor: { layoutVersion: LAYOUT_VERSION, density: "compact", nodes: [] },
  };
}

export function createInstance(cls: ApplianceClass, assembly: ApplicationAssembly): ApplianceInstance {
  const taken = new Set(assembly.spec.appliances.map((a) => a.id));
  const id = uniqueId(cls.id, taken);
  return {
    id,
    name: id,
    classRef: { name: cls.id, version: cls.version },
    resources: { ...cls.resources },
    ports: cls.ports.map((p) => ({ ...p })),
  };
}

export function addAppliance(a: ApplicationAssembly, cls: ApplianceClass, pos: { x: number; y: number }): { assembly: ApplicationAssembly; id: string } {
  const inst = createInstance(cls, a);
  const size = effectiveSize({ ...DENSITY_SIZE[a.editor.density], custom: false }, inst.ports, a.editor.density);
  return {
    id: inst.id,
    assembly: {
      ...a,
      spec: { ...a.spec, appliances: [...a.spec.appliances, inst] },
      editor: { ...a.editor, nodes: [...a.editor.nodes, { id: inst.id, x: Math.round(pos.x), y: Math.round(pos.y), width: size.width, height: size.height, custom: false }] },
    },
  };
}

export function removeAppliances(a: ApplicationAssembly, ids: readonly string[]): ApplicationAssembly {
  const gone = new Set(ids);
  return {
    ...a,
    spec: {
      appliances: a.spec.appliances.filter((x) => !gone.has(x.id)),
      connections: a.spec.connections.filter((c) => !gone.has(c.source.applianceId) && !gone.has(c.target.applianceId)),
    },
    editor: { ...a.editor, nodes: a.editor.nodes.filter((n) => !gone.has(n.id)) },
  };
}

export function moveNodes(a: ApplicationAssembly, moves: ReadonlyMap<string, { x: number; y: number }>): ApplicationAssembly {
  return { ...a, editor: { ...a.editor, nodes: a.editor.nodes.map((n) => { const m = moves.get(n.id); return m ? { ...n, x: Math.round(m.x), y: Math.round(m.y) } : n; }) } };
}

export function resizeNode(a: ApplicationAssembly, id: string, size: { width: number; height: number }, pos?: { x: number; y: number }): ApplicationAssembly {
  const inst = a.spec.appliances.find((x) => x.id === id);
  const minH = inst ? minHeightForPorts(inst.ports) : 96;
  return {
    ...a,
    editor: {
      ...a.editor,
      nodes: a.editor.nodes.map((n) =>
        n.id === id ? { ...n, x: pos ? Math.round(pos.x) : n.x, y: pos ? Math.round(pos.y) : n.y, width: Math.max(128, Math.round(size.width)), height: Math.max(minH, Math.round(size.height)), custom: true } : n,
      ),
    },
  };
}

export function resetNodeSize(a: ApplicationAssembly, id: string): ApplicationAssembly {
  return { ...a, editor: { ...a.editor, nodes: a.editor.nodes.map((n) => (n.id === id ? { ...n, custom: false } : n)) } };
}

export function setDensity(a: ApplicationAssembly, density: Density): ApplicationAssembly {
  const from = DENSITY_SIZE[a.editor.density];
  const size = DENSITY_SIZE[density];
  const fx = size.width / from.width;
  const fy = size.height / from.height;
  return {
    ...a,
    editor: {
      ...a.editor,
      density,
      // Positions scale with the preset so spacing between nodes is preserved. Nodes without a manual
      // size follow the preset; manually resized nodes keep their size.
      nodes: a.editor.nodes.map((n) => ({
        ...n,
        x: Math.round(n.x * fx),
        y: Math.round(n.y * fy),
        ...(n.custom ? {} : { width: size.width, height: size.height }),
      })),
    },
  };
}

function updateAppliance(a: ApplicationAssembly, id: string, f: (x: ApplianceInstance) => ApplianceInstance): ApplicationAssembly {
  return { ...a, spec: { ...a.spec, appliances: a.spec.appliances.map((x) => (x.id === id ? f(x) : x)) } };
}

export function renameAppliance(a: ApplicationAssembly, id: string, name: string): ApplicationAssembly {
  return updateAppliance(a, id, (x) => ({ ...x, name: name.trim().slice(0, 48) || x.name }));
}

export function setResources(a: ApplicationAssembly, id: string, r: Partial<ApplianceInstance["resources"]>): ApplicationAssembly {
  return updateAppliance(a, id, (x) => ({ ...x, resources: { ...x.resources, ...r } }));
}

function growToFit(a: ApplicationAssembly, id: string): ApplicationAssembly {
  const inst = a.spec.appliances.find((x) => x.id === id);
  if (!inst) return a;
  const minH = minHeightForPorts(inst.ports);
  return { ...a, editor: { ...a.editor, nodes: a.editor.nodes.map((n) => (n.id === id && n.height < minH ? { ...n, height: minH } : n)) } };
}

export function addPort(a: ApplicationAssembly, applianceId: string, direction: PortDirection, type: PortType = "tcp"): { assembly: ApplicationAssembly; portId: string } {
  const inst = a.spec.appliances.find((x) => x.id === applianceId);
  const taken = new Set(inst?.ports.map((p) => p.id));
  const base = direction === "input" ? "in" : "out";
  let n = 1;
  while (taken.has(`${base}-${n}`)) n++;
  const portId = `${base}-${n}`;
  const port: Port = { id: portId, name: `${direction === "input" ? "Input" : "Output"} ${n}`, direction, type, optional: true, multiplicity: "many" };
  return { portId, assembly: growToFit(updateAppliance(a, applianceId, (x) => ({ ...x, ports: [...x.ports, port] })), applianceId) };
}

export function updatePort(a: ApplicationAssembly, applianceId: string, portId: string, patch: Partial<Pick<Port, "name" | "type" | "optional" | "multiplicity">>): ApplicationAssembly {
  return updateAppliance(a, applianceId, (x) => ({ ...x, ports: x.ports.map((p) => (p.id === portId ? { ...p, ...patch, name: (patch.name ?? p.name).slice(0, 48) || p.name } : p)) }));
}

/** Removes a port and every connection attached to it. */
export function removePort(a: ApplicationAssembly, applianceId: string, portId: string): ApplicationAssembly {
  const next = updateAppliance(a, applianceId, (x) => ({ ...x, ports: x.ports.filter((p) => p.id !== portId) }));
  return {
    ...next,
    spec: { ...next.spec, connections: next.spec.connections.filter((c) => !((c.source.applianceId === applianceId && c.source.portId === portId) || (c.target.applianceId === applianceId && c.target.portId === portId))) },
  };
}

/** Reorders ports within one side; connections follow because they reference stable port ids. */
export function movePort(a: ApplicationAssembly, applianceId: string, portId: string, delta: -1 | 1): ApplicationAssembly {
  return updateAppliance(a, applianceId, (x) => {
    const p = x.ports.find((q) => q.id === portId);
    if (!p) return x;
    const side = x.ports.filter((q) => q.direction === p.direction);
    const i = side.findIndex((q) => q.id === portId);
    const j = i + delta;
    if (j < 0 || j >= side.length) return x;
    const reordered = [...side];
    [reordered[i], reordered[j]] = [reordered[j]!, reordered[i]!];
    let k = 0;
    return { ...x, ports: x.ports.map((q) => (q.direction === p.direction ? reordered[k++]! : q)) };
  });
}

export function connect(a: ApplicationAssembly, source: Endpoint, target: Endpoint): { assembly: ApplicationAssembly } | { error: { code: string; message: string; hint?: string } } {
  const error = explainConnection(a, source, target);
  if (error) return { error };
  const taken = new Set(a.spec.connections.map((c) => c.id));
  let n = a.spec.connections.length + 1;
  while (taken.has(`conn-${String(n).padStart(2, "0")}`)) n++;
  const conn: Connection = { id: `conn-${String(n).padStart(2, "0")}`, source, target };
  return { assembly: { ...a, spec: { ...a.spec, connections: [...a.spec.connections, conn] } } };
}

export function disconnect(a: ApplicationAssembly, ids: readonly string[]): ApplicationAssembly {
  const gone = new Set(ids);
  return { ...a, spec: { ...a.spec, connections: a.spec.connections.filter((c) => !gone.has(c.id)) } };
}

/**
 * Nudges a desired position diagonally until the new node would not overlap an existing one.
 * Used for click-to-add; drag-and-drop always lands exactly where the user dropped.
 */
export function findFreeSpot(a: ApplicationAssembly, pos: { x: number; y: number }, size: { width: number; height: number }, gap = 16): { x: number; y: number } {
  const hits = (x: number, y: number) =>
    a.editor.nodes.some((n) => x < n.x + n.width + gap && x + size.width + gap > n.x && y < n.y + n.height + gap && y + size.height + gap > n.y);
  let { x, y } = pos;
  for (let i = 0; i < 60 && hits(x, y); i++) {
    x += 28;
    y += 28;
    if (i % 8 === 7) { x -= 8 * 28; y += 12; }
  }
  return { x, y };
}
