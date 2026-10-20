import { ensureNics, parseAssembly, serializeAssembly, type ApplicationAssembly } from "@apelora/assembly-model";
import { lookupClass } from "@apelora/appliance-catalog";

const KEY = "apelora.studio.snapshots.v1";
export const MAX_SNAPSHOTS = 20;
const MAX_TOTAL_CHARS = 3_000_000;

export interface Snapshot { id: string; name: string; at: string; json: string }

export function listSnapshots(): Snapshot[] {
  try {
    const raw = JSON.parse(localStorage.getItem(KEY) ?? "[]") as unknown;
    if (!Array.isArray(raw)) return [];
    return raw.filter((s): s is Snapshot => !!s && typeof s === "object" && typeof (s as Snapshot).id === "string" && typeof (s as Snapshot).json === "string" && typeof (s as Snapshot).name === "string" && typeof (s as Snapshot).at === "string");
  } catch { return []; }
}

function write(list: Snapshot[]): boolean {
  try { localStorage.setItem(KEY, JSON.stringify(list)); return true; } catch { return false; }
}

/** Saves the assembly under a name. The oldest snapshots are dropped when the list or the storage budget is exceeded. */
export function addSnapshot(name: string, a: ApplicationAssembly): { ok: true; list: Snapshot[] } | { ok: false; error: string } {
  const json = serializeAssembly(a);
  if (json.length > 800_000) return { ok: false, error: "This diagram is too large to keep as a snapshot. Export it as a file instead." };
  const snap: Snapshot = { id: `snap-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 6)}`, name: name.trim().slice(0, 60) || "Snapshot", at: new Date().toISOString(), json };
  let list = [...listSnapshots(), snap].slice(-MAX_SNAPSHOTS);
  while (list.length > 1 && list.reduce((n, s) => n + s.json.length, 0) > MAX_TOTAL_CHARS) list = list.slice(1);
  return write(list) ? { ok: true, list } : { ok: false, error: "Browser storage is full or unavailable, so the snapshot was not saved." };
}

export function removeSnapshot(id: string): Snapshot[] {
  const list = listSnapshots().filter((s) => s.id !== id);
  write(list);
  return list;
}

/** Reads a snapshot back through the same validation as an import. */
export function readSnapshot(s: Snapshot): ApplicationAssembly | null {
  const r = parseAssembly(s.json);
  return r.ok ? ensureNics(r.assembly, lookupClass) : null;
}
