import { create } from "zustand";
import { allClasses, lookupClass, setCustomClasses } from "@apelora/module-catalog";
import { ModuleClassSchema, SUBCATEGORIES, effectiveValue, migrateLegacyKeys, slug, validateProperty, type ModuleClass, type ConfigValue, type Port, type Property } from "@apelora/assembly-model";

const KEY = "apelora.studio.catalog.v1";

export interface DraftPort { key: string; /** The network interface every module has; cannot be removed or retyped. */ locked?: boolean; name: string; direction: Port["direction"]; type: Port["type"]; optional: boolean; multiplicity: Port["multiplicity"]; description: string }
export interface DraftProp { key: string; /** Stable id kept when editing, so saved configuration values keep matching. */ id?: string; label: string; type: Property["type"]; default: string; required: boolean; group: string; description: string; options: string; min: string; max: string }

export interface Draft {
  /** Set when editing an existing custom class. */
  editingId?: string;
  name: string;
  category: ModuleClass["category"];
  /** Where it sits inside its category in the catalog menu; none for Custom. */
  subcategory?: string;
  summary: string;
  icon: string;
  cpu: number;
  memoryMiB: number;
  ports: DraftPort[];
  properties: DraftProp[];
}

function load(): ModuleClass[] {
  try {
    const raw = JSON.parse(localStorage.getItem(KEY) ?? "[]") as unknown;
    if (!Array.isArray(raw)) return [];
    return raw.flatMap((c) => { const r = ModuleClassSchema.safeParse(migrateLegacyKeys(c)); return r.success ? [{ ...r.data, custom: true }] : []; });
  } catch { return []; }
}
const persist = (list: ModuleClass[]) => { try { localStorage.setItem(KEY, JSON.stringify(list)); } catch { /* storage unavailable */ } };

interface CatalogState {
  classes: ModuleClass[];
  draft: Draft | null;
  openDesigner: (d: Draft) => void;
  closeDesigner: () => void;
  save: (c: ModuleClass) => void;
  remove: (id: string) => void;
  /** Registers classes embedded in an imported file that this browser does not know yet. Returns how many were added. */
  addMissing: (list: readonly ModuleClass[]) => number;
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
export const newPropKey = () => `c${keySeq++}`;

export function emptyProp(): DraftProp {
  return { key: newPropKey(), label: "", type: "string", default: "", required: false, group: "", description: "", options: "", min: "", max: "" };
}

function toDraftProps(list: readonly Property[] | undefined, config?: Readonly<Record<string, ConfigValue>>): DraftProp[] {
  return (list ?? []).map((p) => ({
    key: newPropKey(), id: p.id, label: p.label, type: p.type, default: String(effectiveValue(p, config) ?? ""), required: p.required,
    group: p.group ?? "", description: p.description ?? "", options: (p.options ?? []).join(", "), min: p.min === undefined ? "" : String(p.min), max: p.max === undefined ? "" : String(p.max),
  }));
}

const NIC_DESCRIPTION = "Network interface (NIC). Plug into a network or a network module; unplugged, it joins the default network.";
export const nicDraft = (): DraftPort => ({ key: newPortKey(), locked: true, name: "net", direction: "input", type: "network", optional: true, multiplicity: "one", description: NIC_DESCRIPTION });

export function emptyDraft(): Draft {
  return {
    name: "", category: "Custom", summary: "", icon: "generic-box", cpu: 1, memoryMiB: 512,
    ports: [
      { key: newPortKey(), name: "in", direction: "input", type: "tcp", optional: false, multiplicity: "many", description: "" },
      { key: newPortKey(), name: "out", direction: "output", type: "tcp", optional: true, multiplicity: "many", description: "" },
      nicDraft(),
    ],
    properties: [],
  };
}

export function draftFromClass(c: ModuleClass, editing: boolean): Draft {
  return {
    ...(editing ? { editingId: c.id } : {}),
    name: editing ? c.displayName : `${c.displayName} (custom)`,
    category: editing ? c.category : "Custom",
    ...(editing && c.subcategory ? { subcategory: c.subcategory } : {}),
    summary: c.summary,
    icon: c.iconAssetPath,
    cpu: c.resources.cpu,
    memoryMiB: c.resources.memoryMiB,
    ports: c.ports.map((p) => ({ key: newPortKey(), ...(p.id === "net" && p.direction === "input" ? { locked: true } : {}), name: p.name, direction: p.direction, type: p.type, optional: p.optional, multiplicity: p.multiplicity, description: p.description ?? "" })),
    properties: toDraftProps(c.properties),
  };
}

export function draftFromPorts(name: string, ports: readonly Port[], res: { cpu: number; memoryMiB: number }, icon: string, props?: readonly Property[], config?: Readonly<Record<string, ConfigValue>>): Draft {
  return {
    name: `${name}-class`, category: "Custom", summary: "", icon, cpu: res.cpu, memoryMiB: res.memoryMiB,
    ports: ports.map((p) => ({ key: newPortKey(), ...(p.id === "net" && p.direction === "input" ? { locked: true } : {}), name: p.name, direction: p.direction, type: p.type, optional: p.optional, multiplicity: p.multiplicity, description: p.description ?? "" })),
    properties: toDraftProps(props, config),
  };
}

/** Builds and validates a class from a draft. Port ids are derived from names and kept unique. */
export function classFromDraft(d: Draft): { ok: true; cls: ModuleClass } | { ok: false; errors: string[] } {
  const errors: string[] = [];
  const name = d.name.trim();
  if (!name) errors.push("Give the module a name.");
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
  if (!ports.some((p) => p.id === "net")) ports.push({ id: "net", name: "net", direction: "input", type: "network", optional: true, multiplicity: "one", description: NIC_DESCRIPTION });
  if (ports.length > 24) errors.push("At most 24 ports are supported.");
  const propIds = new Set<string>();
  const properties: Property[] = d.properties.map((p) => {
    const label = p.label.trim();
    if (!label) errors.push("Every configuration field needs a label.");
    const base = p.id ?? slug(label || "field");
    let id = base;
    let i = 2;
    while (propIds.has(id)) id = `${base}-${i++}`;
    propIds.add(id);
    const options = p.type === "enum" ? p.options.split(",").map((o) => o.trim()).filter(Boolean) : undefined;
    if (p.type === "enum" && (!options || options.length < 2)) errors.push(`"${label || "field"}": list at least two options, separated by commas.`);
    const num = (v: string) => (v.trim() === "" ? undefined : Number(v));
    const min = p.type === "int" ? num(p.min) : undefined;
    const max = p.type === "int" ? num(p.max) : undefined;
    if ((min !== undefined && !Number.isFinite(min)) || (max !== undefined && !Number.isFinite(max))) errors.push(`"${label || "field"}": min and max must be numbers.`);
    if (min !== undefined && max !== undefined && min > max) errors.push(`"${label || "field"}": min is greater than max.`);
    let def: ConfigValue | undefined;
    const raw = p.default.trim();
    if (raw !== "") def = p.type === "int" ? Number(raw) : p.type === "bool" ? raw === "true" : raw;
    const built: Property = { id, label: label || "field", type: p.type, required: p.required, ...(def !== undefined ? { default: def } : {}), ...(p.group.trim() ? { group: p.group.trim().slice(0, 32) } : {}), ...(p.description.trim() ? { description: p.description.trim().slice(0, 160) } : {}), ...(options ? { options } : {}), ...(min !== undefined && Number.isFinite(min) ? { min } : {}), ...(max !== undefined && Number.isFinite(max) ? { max } : {}) };
    if (def !== undefined || p.required) { const bad = validateProperty(built, def); if (bad && label) errors.push(`"${label}" default: ${bad.message}`); }
    return built;
  });
  if (!(d.cpu >= 1)) errors.push("vCPU must be at least 1.");
  if (!(d.memoryMiB >= 64)) errors.push("Memory must be at least 64 MiB.");
  if (errors.length) return { ok: false, errors };

  let id = d.editingId ?? `custom-${slug(name)}`;
  if (!d.editingId) { let i = 2; const base = id; while (allClasses().some((c) => c.id === id)) id = `${base}-${i++}`; }
  const r = ModuleClassSchema.safeParse({
    id, version: "1.0.0", displayName: name.slice(0, 48), category: d.category, ...(d.subcategory && SUBCATEGORIES[d.category].includes(d.subcategory) ? { subcategory: d.subcategory } : {}), summary: d.summary.trim().slice(0, 200), iconAssetPath: d.icon,
    runtimeImage: { ref: null }, architectures: ["x86_64", "aarch64"], ports, resources: { cpu: d.cpu, memoryMiB: d.memoryMiB }, ...(properties.length ? { properties } : {}), custom: true,
  });
  return r.success ? { ok: true, cls: r.data } : { ok: false, errors: r.error.issues.slice(0, 5).map((i) => `${i.path.join(".")}: ${i.message}`) };
}
