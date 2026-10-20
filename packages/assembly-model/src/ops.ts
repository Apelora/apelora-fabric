import { API_VERSION, ASSEMBLY_KIND, LAYOUT_VERSION } from "./constants";
import { DENSITY_SIZE, effectiveSize, minHeightForPorts } from "./geometry";
import { explainConnection } from "./validate";
import { MAX_REPLICAS } from "./schema";
import type { ApplianceClass, ApplicationAssembly, ApplianceInstance, ConfigValue, Connection, Density, Endpoint, Port, PortDirection, PortType } from "./schema";

function dropLinks(a: ApplicationAssembly, pred: (l: NonNullable<ApplicationAssembly["spec"]["links"]>[number]) => boolean): ApplicationAssembly["spec"]["links"] {
  const next = (a.spec.links ?? []).filter((l) => !pred(l));
  return next.length ? next : undefined;
}

export function slug(input: string): string {
  const s = input.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "");
  return /^[a-z]/.test(s) ? s.slice(0, 40) : `x-${s}`.slice(0, 40);
}

export function uniqueId(base: string, taken: ReadonlySet<string>): string {
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
      ...a.spec,
      appliances: a.spec.appliances.filter((x) => !gone.has(x.id)),
      connections: a.spec.connections.filter((c) => !gone.has(c.source.applianceId) && !gone.has(c.target.applianceId)),
      links: dropLinks(a, (l) => gone.has(l.endpoint.applianceId)),
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
      shapes: a.editor.shapes?.map((sh) => ({ ...sh, x: Math.round(sh.x * fx), y: Math.round(sh.y * fy), width: Math.round(sh.width * fx), height: Math.round(sh.height * fy) })),
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

/** Sets how many instances an appliance runs (1–9; 1 is stored as absent). */
export function setReplicas(a: ApplicationAssembly, id: string, n: number): ApplicationAssembly {
  const replicas = Math.min(MAX_REPLICAS, Math.max(1, Math.round(n)));
  return updateAppliance(a, id, ({ replicas: _old, ...x }) => (replicas === 1 ? x : { ...x, replicas }));
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
    spec: { ...next.spec, links: dropLinks(next, (l) => l.endpoint.applianceId === applianceId && l.endpoint.portId === portId), connections: next.spec.connections.filter((c) => !((c.source.applianceId === applianceId && c.source.portId === portId) || (c.target.applianceId === applianceId && c.target.portId === portId))) },
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

/** Moves one end (or both) of an existing connection to different ports, keeping its id. */
export function reconnect(a: ApplicationAssembly, connectionId: string, source: Endpoint, target: Endpoint): { assembly: ApplicationAssembly } | { error: { code: string; message: string; hint?: string } } {
  if (!a.spec.connections.some((c) => c.id === connectionId)) return { error: { code: "connection-missing", message: "That connection no longer exists." } };
  const error = explainConnection(a, source, target, connectionId);
  if (error) return { error };
  return { assembly: { ...a, spec: { ...a.spec, connections: a.spec.connections.map((c) => (c.id === connectionId ? { ...c, source, target } : c)) } } };
}

/** Removes every cable attached to one port. */
export function disconnectPort(a: ApplicationAssembly, applianceId: string, portId: string): ApplicationAssembly {
  return { ...a, spec: { ...a.spec, links: dropLinks(a, (l) => l.endpoint.applianceId === applianceId && l.endpoint.portId === portId), connections: a.spec.connections.filter((c) => !((c.source.applianceId === applianceId && c.source.portId === portId) || (c.target.applianceId === applianceId && c.target.portId === portId))) } };
}

/** Removes every cable attached to an appliance. */
export function disconnectAppliance(a: ApplicationAssembly, applianceId: string): ApplicationAssembly {
  return { ...a, spec: { ...a.spec, links: dropLinks(a, (l) => l.endpoint.applianceId === applianceId), connections: a.spec.connections.filter((c) => c.source.applianceId !== applianceId && c.target.applianceId !== applianceId) } };
}

/** A copyable slice of an assembly: appliances, their layout and the cables between them. */
export interface Subset {
  appliances: ApplianceInstance[];
  nodes: ApplicationAssembly["editor"]["nodes"];
  connections: Connection[];
}

export function extractSubset(a: ApplicationAssembly, ids: readonly string[]): Subset {
  const keep = new Set(ids);
  return {
    appliances: a.spec.appliances.filter((x) => keep.has(x.id)),
    nodes: a.editor.nodes.filter((n) => keep.has(n.id)),
    connections: a.spec.connections.filter((c) => keep.has(c.source.applianceId) && keep.has(c.target.applianceId)),
  };
}

/** Inserts a copied subset with fresh ids, offset by `delta`. Only cables between the copied appliances are kept. */
export function insertSubset(a: ApplicationAssembly, s: Subset, delta: { x: number; y: number }): { assembly: ApplicationAssembly; ids: string[] } {
  const taken = new Set(a.spec.appliances.map((x) => x.id));
  const idMap = new Map<string, string>();
  for (const x of s.appliances) {
    const id = uniqueId(x.classRef.name, taken);
    taken.add(id);
    idMap.set(x.id, id);
  }
  const connTaken = new Set(a.spec.connections.map((c) => c.id));
  let n = a.spec.connections.length;
  const conns: Connection[] = s.connections.map((c) => {
    let id: string;
    do id = `conn-${String(++n).padStart(2, "0")}`; while (connTaken.has(id));
    connTaken.add(id);
    return { id, source: { ...c.source, applianceId: idMap.get(c.source.applianceId)! }, target: { ...c.target, applianceId: idMap.get(c.target.applianceId)! } };
  });
  return {
    ids: [...idMap.values()],
    assembly: {
      ...a,
      spec: {
        ...a.spec,
        appliances: [...a.spec.appliances, ...s.appliances.map((x) => ({ ...x, id: idMap.get(x.id)!, name: idMap.get(x.id)!, ports: x.ports.map((p) => ({ ...p })), resources: { ...x.resources } }))],
        connections: [...a.spec.connections, ...conns],
      },
      editor: { ...a.editor, nodes: [...a.editor.nodes, ...s.nodes.map((nd) => ({ ...nd, id: idMap.get(nd.id)!, x: Math.round(nd.x + delta.x), y: Math.round(nd.y + delta.y) }))] },
    },
  };
}

export type AlignMode = "left" | "right" | "top" | "bottom" | "center-x" | "center-y";

export function alignNodes(a: ApplicationAssembly, ids: readonly string[], mode: AlignMode): ApplicationAssembly {
  const sel = a.editor.nodes.filter((n) => ids.includes(n.id));
  if (sel.length < 2) return a;
  const minX = Math.min(...sel.map((n) => n.x));
  const maxR = Math.max(...sel.map((n) => n.x + n.width));
  const minY = Math.min(...sel.map((n) => n.y));
  const maxB = Math.max(...sel.map((n) => n.y + n.height));
  return {
    ...a,
    editor: {
      ...a.editor,
      nodes: a.editor.nodes.map((n) => {
        if (!ids.includes(n.id)) return n;
        switch (mode) {
          case "left": return { ...n, x: minX };
          case "right": return { ...n, x: maxR - n.width };
          case "top": return { ...n, y: minY };
          case "bottom": return { ...n, y: maxB - n.height };
          case "center-x": return { ...n, x: Math.round((minX + maxR) / 2 - n.width / 2) };
          case "center-y": return { ...n, y: Math.round((minY + maxB) / 2 - n.height / 2) };
        }
      }),
    },
  };
}

/** Sets one configuration override; `undefined` removes it so the class default applies again. */
export function setConfig(a: ApplicationAssembly, applianceId: string, key: string, value: ConfigValue | undefined): ApplicationAssembly {
  return updateAppliance(a, applianceId, (x) => {
    const next = { ...(x.config ?? {}) };
    if (value === undefined) delete next[key];
    else next[key] = value;
    const { config: _old, ...rest } = x;
    void _old;
    return Object.keys(next).length ? { ...rest, config: next } : rest;
  });
}

export function resetConfig(a: ApplicationAssembly, applianceId: string): ApplicationAssembly {
  return updateAppliance(a, applianceId, (x) => { const { config: _old, ...rest } = x; void _old; return rest; });
}

/**
 * Brings older assemblies up to date: any appliance whose class defines a `net` interface but whose instance
 * lacks it gets one. Returns the same object when nothing changed.
 */
export function ensureNics(a: ApplicationAssembly, lookup: (name: string, version: string) => ApplianceClass | undefined): ApplicationAssembly {
  let changed = false;
  const appliances = a.spec.appliances.map((x) => {
    if (x.ports.some((p) => p.id === "net")) return x;
    const nic = lookup(x.classRef.name, x.classRef.version)?.ports.find((p) => p.id === "net");
    if (!nic) return x;
    changed = true;
    return { ...x, ports: [...x.ports, { ...nic }] };
  });
  if (!changed) return a;
  return growAll({ ...a, spec: { ...a.spec, appliances } });
}

function growAll(a: ApplicationAssembly): ApplicationAssembly {
  return { ...a, editor: { ...a.editor, nodes: a.editor.nodes.map((n) => { const inst = a.spec.appliances.find((x) => x.id === n.id); const min = inst ? minHeightForPorts(inst.ports) : 0; return n.height < min ? { ...n, height: min } : n; }) } };
}
