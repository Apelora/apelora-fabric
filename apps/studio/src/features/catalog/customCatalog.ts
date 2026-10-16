import { create } from "zustand";
import { allClasses, lookupClass, setCustomClasses } from "@apelora/appliance-catalog";
import { ApplianceClassSchema, slug, type ApplianceClass, type Port } from "@apelora/assembly-model";

const KEY = "apelora.studio.catalog.v1";

export interface DraftPort { key: string; name: string; direction: Port["direction"]; type: Port["type"]; optional: boolean; multiplicity: Port["multiplicity"]; description: string }
export interface Draft {
  /** Set when editing an existing custom class. */
  editingId?: string;
  name: string;
  category: ApplianceClass["category"];
  summary: string;
  icon: string;
  cpu: number;
  memoryMiB: number;
  ports: DraftPort[];
}

function load(): ApplianceClass[] {
  try {
    const raw = JSON.parse(localStorage.getItem(KEY) ?? "[]") as unknown;
    if (!Array.isArray(raw)) return [];
    return raw.flatMap((c) => { const r = ApplianceClassSchema.safeParse(c); return r.success ? [{ ...r.data, custom: true }] : []; });
  } catch { return []; }
}
const persist = (list: ApplianceClass[]) => { try { localStorage.setItem(KEY, JSON.stringify(list)); } catch { /* storage unavailable */ } };

interface CatalogState {
  classes: ApplianceClass[];
  draft: Draft | null;
  openDesigner: (d: Draft) => void;
  closeDesigner: () => void;
  save: (c: ApplianceClass) => void;
  remove: (id: string) => void;
  /** Registers classes embedded in an imported file that this browser does not know yet. Returns how many were added. */
  addMissing: (list: readonly ApplianceClass[]) => number;
}

const initial = load();
setCustomClasses(initial);

export const useCustomCatalog = create<CatalogState>()((set, get) => ({
  classes: initial,
  draft: null,
  openDesigner: (draft) => set({ draft }),
  closeDesigner: () => set({ draft: null }),
  save: (c) => {
    const rest = get().classes.filter((x) => !(x.id === c.id && x.version === c.version));
    const next = [...rest, { ...c, custom: true }];
    setCustomClasses(next); persist(next); set({ classes: next });
  },
  remove: (id) => { const next = get().classes.filter((x) => x.id !== id); setCustomClasses(next); persist(next); set({ classes: next }); },
  addMissing: (list) => {
    const fresh = list.filter((c) => !lookupClass(c.id, c.version));
    if (!fresh.length) return 0;
    const next = [...get().classes, ...fresh.map((c) => ({ ...c, custom: true }))];
    setCustomClasses(next); persist(next); set({ classes: next });
    return fresh.length;
  },
}));

export const GENERIC_NAME_HINT = "e.g. cache-node";

let keySeq = 1;
export const newPortKey = () => `p${keySeq++}`;

export function emptyDraft(): Draft {
  return {
    name: "", category: "Custom", summary: "", icon: "generic-box", cpu: 1, memoryMiB: 512,
    ports: [
      { key: newPortKey(), name: "in", direction: "input", type: "tcp", optional: false, multiplicity: "many", description: "" },
      { key: newPortKey(), name: "out", direction: "output", type: "tcp", optional: true, multiplicity: "many", description: "" },
    ],
  };
}

export function draftFromClass(c: ApplianceClass, editing: boolean): Draft {
  return {
    ...(editing ? { editingId: c.id } : {}),
    name: editing ? c.displayName : `${c.displayName} (custom)`,
    category: editing ? c.category : "Custom",
    summary: c.summary,
    icon: c.iconAssetPath,
    cpu: c.resources.cpu,
    memoryMiB: c.resources.memoryMiB,
    ports: c.ports.map((p) => ({ key: newPortKey(), name: p.name, direction: p.direction, type: p.type, optional: p.optional, multiplicity: p.multiplicity, description: p.description ?? "" })),
  };
}

export function draftFromPorts(name: string, ports: readonly Port[], res: { cpu: number; memoryMiB: number }, icon: string): Draft {
  return {
    name: `${name}-class`, category: "Custom", summary: "", icon, cpu: res.cpu, memoryMiB: res.memoryMiB,
    ports: ports.map((p) => ({ key: newPortKey(), name: p.name, direction: p.direction, type: p.type, optional: p.optional, multiplicity: p.multiplicity, description: p.description ?? "" })),
  };
}

/** Builds and validates a class from a draft. Port ids are derived from names and kept unique. */
export function classFromDraft(d: Draft): { ok: true; cls: ApplianceClass } | { ok: false; errors: string[] } {
  const errors: string[] = [];
  const name = d.name.trim();
  if (!name) errors.push("Give the appliance a name.");
  const names = new Set<string>();
  const ids = new Set<string>();
  const ports: Port[] = d.ports.map((p) => {
    const n = p.name.trim();
    if (!n) errors.push("Every port needs a name.");
    if (names.has(`${p.direction}:${n.toLowerCase()}`)) errors.push(`Two ${p.direction} ports are both called "${n}".`);
    names.add(`${p.direction}:${n.toLowerCase()}`);
    let id = slug(n || "port");
    let i = 2;
    while (ids.has(id)) id = `${slug(n || "port")}-${i++}`;
    ids.add(id);
    return { id, name: n || "port", direction: p.direction, type: p.type, optional: p.optional, multiplicity: p.multiplicity, ...(p.description.trim() ? { description: p.description.trim().slice(0, 160) } : {}) };
  });
  if (ports.length > 24) errors.push("At most 24 ports are supported.");
  if (!(d.cpu >= 1)) errors.push("vCPU must be at least 1.");
  if (!(d.memoryMiB >= 64)) errors.push("Memory must be at least 64 MiB.");
  if (errors.length) return { ok: false, errors };

  let id = d.editingId ?? `custom-${slug(name)}`;
  if (!d.editingId) { let i = 2; const base = id; while (allClasses().some((c) => c.id === id)) id = `${base}-${i++}`; }
  const r = ApplianceClassSchema.safeParse({
    id, version: "1.0.0", displayName: name.slice(0, 48), category: d.category, summary: d.summary.trim().slice(0, 200), iconAssetPath: d.icon,
    runtimeImage: { ref: null }, architectures: ["x86_64", "aarch64"], ports, resources: { cpu: d.cpu, memoryMiB: d.memoryMiB }, custom: true,
  });
  return r.success ? { ok: true, cls: r.data } : { ok: false, errors: r.error.issues.slice(0, 5).map((i) => `${i.path.join(".")}: ${i.message}`) };
}
