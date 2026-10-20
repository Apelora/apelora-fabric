import { minHeightForPorts } from "./geometry";
import { slug, uniqueId } from "./ops";
import type { ModuleClass, ApplicationAssembly, ModuleInstance, Connection, Endpoint, LinkTag, Port } from "./schema";

export interface ExposeChoice {
  /** The inner terminal that becomes (part of) a port of the component. */
  inner: Endpoint;
  /** Name shown on the component's port. */
  name: string;
}

export interface GroupCandidate {
  module: ModuleInstance;
  port: Port;
  /** True when a cable (or tag) links this terminal to something outside the selection; it must be exposed or the link is lost. */
  external: boolean;
}

const same = (a: Endpoint, b: Endpoint) => a.moduleId === b.moduleId && a.portId === b.portId;

/** Terminals of the selected modules that a component could expose (the `net` NICs are handled separately). */
export function groupCandidates(a: ApplicationAssembly, ids: readonly string[]): GroupCandidate[] {
  const inside = new Set(ids);
  const external = (e: Endpoint) => a.spec.connections.some((c) => (same(c.source, e) && !inside.has(c.target.moduleId)) || (same(c.target, e) && !inside.has(c.source.moduleId)));
  return a.spec.modules.filter((x) => inside.has(x.id)).flatMap((app) =>
    app.ports.filter((p) => p.id !== "net").map((p) => ({ module: app, port: p, external: external({ moduleId: app.id, portId: p.id }) })),
  );
}

const NIC: Port = { id: "net", name: "net", direction: "input", type: "network", optional: true, multiplicity: "one", description: "Network interface (NIC) of the whole component: plugs every inner module into the network." };

export type GroupResult = { ok: true; assembly: ApplicationAssembly; cls: ModuleClass; instanceId: string; dropped: number } | { ok: false; errors: string[] };

/**
 * Packages the selected modules into one reusable component. `exposes` lists the inner terminals offered as ports of
 * the new class (names must be unique per direction). External cables to exposed terminals are re-plugged to the component;
 * cables to terminals that are not exposed are dropped (counted in `dropped`).
 */
export function groupModules(a: ApplicationAssembly, ids: readonly string[], className: string, exposes: readonly ExposeChoice[], existingClassIds: ReadonlySet<string>): GroupResult {
  const errors: string[] = [];
  const inside = new Set(ids);
  const members = a.spec.modules.filter((x) => inside.has(x.id));
  if (members.length < 2) errors.push("Select at least two modules to make a component.");
  const name = className.trim();
  if (!name) errors.push("Give the component a name.");

  const ports: Port[] = [];
  const mapping = new Map<string, Endpoint>();
  const seen = new Set<string>();
  for (const e of exposes) {
    const app = members.find((m) => m.id === e.inner.moduleId);
    const p = app?.ports.find((q) => q.id === e.inner.portId);
    if (!app || !p) { errors.push("An exposed terminal no longer exists."); continue; }
    const n = e.name.trim();
    if (!n) { errors.push(`Name the exposed port for ${app.name}.${p.name}.`); continue; }
    const key = `${p.direction}:${n.toLowerCase()}`;
    if (seen.has(key)) { errors.push(`Two ${p.direction} ports are called "${n}".`); continue; }
    seen.add(key);
    let id = slug(n);
    let i = 2;
    while (ports.some((q) => q.id === id) || id === "net") id = `${slug(n)}-${i++}`;
    ports.push({ id, name: n, direction: p.direction, type: p.type, optional: p.optional, multiplicity: p.multiplicity, ...(p.description ? { description: p.description } : {}) });
    mapping.set(`${app.id}:${p.id}`, { moduleId: "", portId: id });
  }
  if (errors.length) return { ok: false, errors };

  ports.push({ ...NIC });
  const innerNics: Endpoint[] = members.filter((m) => m.ports.some((p) => p.id === "net")).map((m) => ({ moduleId: m.id, portId: "net" }));

  const innerLayout = a.editor.nodes.filter((n) => inside.has(n.id));
  const minX = Math.min(...innerLayout.map((n) => n.x));
  const minY = Math.min(...innerLayout.map((n) => n.y));
  const maxX = Math.max(...innerLayout.map((n) => n.x + n.width));
  const maxY = Math.max(...innerLayout.map((n) => n.y + n.height));

  let id = `custom-${slug(name)}`;
  { let i = 2; const base = id; while (existingClassIds.has(id)) id = `${base}-${i++}`; }

  const exposeList = [...mapping.entries()].map(([k, v]) => {
    const [moduleId, portId] = k.split(":") as [string, string];
    return { port: v.portId, inner: [{ moduleId, portId }] };
  });
  const cls: ModuleClass = {
    id, version: "1.0.0", displayName: name.slice(0, 48), category: "Custom",
    summary: `Component of ${members.length} modules.`, iconAssetPath: "generic-box",
    runtimeImage: { ref: null }, architectures: ["x86_64", "aarch64"], ports,
    resources: { cpu: members.reduce((s, m) => s + m.resources.cpu, 0), memoryMiB: members.reduce((s, m) => s + m.resources.memoryMiB, 0) },
    custom: true,
    composition: {
      modules: members.map((m) => ({ ...m, ports: m.ports.map((p) => ({ ...p })), resources: { ...m.resources } })),
      connections: a.spec.connections.filter((c) => inside.has(c.source.moduleId) && inside.has(c.target.moduleId)),
      links: (a.spec.links ?? []).filter((l) => inside.has(l.endpoint.moduleId)),
      layout: innerLayout.map((n) => ({ ...n, x: n.x - minX, y: n.y - minY })),
      exposes: [...exposeList, ...(innerNics.length ? [{ port: "net", inner: innerNics }] : [])],
    },
  };

  // Replace the selection by one instance and re-plug external cables.
  const taken = new Set(a.spec.modules.map((x) => x.id));
  const instId = uniqueId(cls.id, taken);
  const outer = a.spec.connections.filter((c) => !(inside.has(c.source.moduleId) && inside.has(c.target.moduleId)));
  let dropped = 0;
  const rewired: Connection[] = [];
  for (const c of outer) {
    const sIn = inside.has(c.source.moduleId);
    const tIn = inside.has(c.target.moduleId);
    if (!sIn && !tIn) { rewired.push(c); continue; }
    const inner = sIn ? c.source : c.target;
    const innerPort = members.find((m) => m.id === inner.moduleId)?.ports.find((p) => p.id === inner.portId);
    const m = innerPort?.id === "net" ? { moduleId: "", portId: "net" } : mapping.get(`${inner.moduleId}:${inner.portId}`);
    if (!m) { dropped++; continue; }
    const ep: Endpoint = { moduleId: instId, portId: m.portId };
    rewired.push(sIn ? { ...c, source: ep } : { ...c, target: ep });
  }
  // NIC cables from several inner NICs to the same network collapse into one cable on the component's net.
  const seenNic = new Set<string>();
  const connections = rewired.filter((c) => {
    if (c.target.moduleId !== instId || c.target.portId !== "net") return true;
    const key = `${c.source.moduleId}:${c.source.portId}`;
    if (seenNic.has(key)) { dropped++; return false; }
    seenNic.add(key);
    return true;
  });

  const instance: ModuleInstance = { id: instId, name: instId, classRef: { name: cls.id, version: cls.version }, resources: { ...cls.resources }, ports: ports.map((p) => ({ ...p })) };
  const nodes = a.editor.nodes.filter((n) => !inside.has(n.id));
  const size = a.editor.density === "compact" ? { width: 128, height: 96 } : a.editor.density === "comfortable" ? { width: 152, height: 112 } : { width: 178, height: 150 };
  nodes.push({ id: instId, x: Math.round((minX + maxX) / 2 - size.width / 2), y: Math.round((minY + maxY) / 2 - size.height / 2), width: size.width, height: Math.max(size.height, minHeightForPorts(ports)), custom: false });
  const links = (a.spec.links ?? []).filter((l) => !inside.has(l.endpoint.moduleId));
  // If every inner NIC joins the same named network, the component's own NIC joins it too.
  const nicTags = new Set(innerNics.map((e) => (a.spec.links ?? []).find((l) => same(l.endpoint, e))?.tag ?? ""));
  if (innerNics.length > 0 && nicTags.size === 1 && !nicTags.has("")) {
    const taken = new Set(links.map((l) => l.id));
    links.push({ id: uniqueId("tag", taken), tag: [...nicTags][0]!, endpoint: { moduleId: instId, portId: "net" } });
  }
  return {
    ok: true, cls, instanceId: instId, dropped,
    assembly: {
      ...a,
      spec: { ...a.spec, modules: [...a.spec.modules.filter((x) => !inside.has(x.id)), instance], connections, links: links.length ? links : undefined },
      editor: { ...a.editor, nodes },
    },
  };
}

/** Expands a component instance back into its modules, re-plugging its outside cables to the inner terminals. */
export function ungroupModule(a: ApplicationAssembly, instanceId: string, cls: ModuleClass): { assembly: ApplicationAssembly; ids: string[] } | { error: string } {
  const comp = cls.composition;
  const node = a.editor.nodes.find((n) => n.id === instanceId);
  if (!comp || !node) return { error: "This module is not a component." };
  const taken = new Set(a.spec.modules.map((x) => x.id));
  taken.delete(instanceId);
  const idMap = new Map<string, string>();
  for (const x of comp.modules) { const id = uniqueId(x.classRef.name, taken); taken.add(id); idMap.set(x.id, id); }
  const remap = (e: Endpoint): Endpoint => ({ ...e, moduleId: idMap.get(e.moduleId)! });
  const inner: ModuleInstance[] = comp.modules.map((x) => ({ ...x, id: idMap.get(x.id)!, name: idMap.get(x.id)!, ports: x.ports.map((p) => ({ ...p })), resources: { ...x.resources } }));
  const layout = comp.layout.map((n) => ({ ...n, id: idMap.get(n.id)!, x: Math.round(node.x + n.x), y: Math.round(node.y + n.y) }));

  const connTaken = new Set(a.spec.connections.map((c) => c.id));
  let n = a.spec.connections.length;
  const fresh = () => { let id: string; do id = `conn-${String(++n).padStart(2, "0")}`; while (connTaken.has(id)); connTaken.add(id); return id; };
  const internal: Connection[] = comp.connections.map((c) => ({ id: fresh(), source: remap(c.source), target: remap(c.target) }));
  const outside: Connection[] = [];
  for (const c of a.spec.connections) {
    const sIs = c.source.moduleId === instanceId;
    const tIs = c.target.moduleId === instanceId;
    if (!sIs && !tIs) { outside.push(c); continue; }
    const ex = comp.exposes.find((e) => e.port === (sIs ? c.source.portId : c.target.portId));
    if (!ex) continue;
    ex.inner.forEach((target, i) => outside.push({ id: i === 0 ? c.id : fresh(), source: sIs ? remap(target) : c.source, target: tIs ? remap(target) : c.target }));
  }
  const netTag = (a.spec.links ?? []).find((l) => l.endpoint.moduleId === instanceId && l.endpoint.portId === "net")?.tag;
  const nicExpose = comp.exposes.find((e) => e.port === "net")?.inner ?? [];
  const isInnerNic = (e: Endpoint) => nicExpose.some((x) => same(x, e));
  const links: LinkTag[] = (a.spec.links ?? []).filter((l) => l.endpoint.moduleId !== instanceId);
  const linkTaken = new Set(links.map((l) => l.id));
  let ln = links.length;
  if (netTag) for (const e of nicExpose) { let id: string; do id = `tag-${String(++ln).padStart(2, "0")}`; while (linkTaken.has(id)); linkTaken.add(id); links.push({ id, tag: netTag, endpoint: remap(e) }); }
  for (const l of (comp.links ?? []).filter((x) => !netTag || !isInnerNic(x.endpoint))) { let id: string; do id = `tag-${String(++ln).padStart(2, "0")}`; while (linkTaken.has(id)); linkTaken.add(id); links.push({ id, tag: l.tag, endpoint: remap(l.endpoint) }); }
  return {
    ids: [...idMap.values()],
    assembly: {
      ...a,
      spec: { ...a.spec, modules: [...a.spec.modules.filter((x) => x.id !== instanceId), ...inner], connections: [...outside, ...internal], links: links.length ? links : undefined },
      editor: { ...a.editor, nodes: [...a.editor.nodes.filter((x) => x.id !== instanceId), ...layout] },
    },
  };
}

/** Custom classes an assembly depends on, including those used inside components, so exports are self-contained. */
export function usedCustomClasses(a: ApplicationAssembly, lookup: (name: string, version: string) => ModuleClass | undefined): ModuleClass[] {
  const found = new Map<string, ModuleClass>();
  const visit = (instances: readonly ModuleInstance[]) => {
    for (const x of instances) {
      const c = lookup(x.classRef.name, x.classRef.version);
      const key = `${x.classRef.name}@${x.classRef.version}`;
      if (!c?.custom || found.has(key)) continue;
      found.set(key, c);
      if (c.composition) visit(c.composition.modules);
    }
  };
  visit(a.spec.modules);
  return [...found.values()];
}
