import { ApplicationAssemblySchema, type ApplicationAssembly } from "./schema";

function canonical(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(canonical);
  if (value && typeof value === "object") {
    return Object.fromEntries(Object.entries(value as Record<string, unknown>).sort(([x], [y]) => (x < y ? -1 : x > y ? 1 : 0)).map(([k, v]) => [k, canonical(v)]));
  }
  return value;
}

/** Deterministic export: sorted keys, collections sorted by id, trailing newline. */
export function serializeAssembly(a: ApplicationAssembly): string {
  const byId = <T extends { id: string }>(xs: readonly T[]) => [...xs].sort((p, q) => (p.id < q.id ? -1 : p.id > q.id ? 1 : 0));
  const sorted: ApplicationAssembly = {
    ...a,
    spec: { modules: byId(a.spec.modules), connections: byId(a.spec.connections), ...(a.spec.links?.length ? { links: byId(a.spec.links) } : {}), ...(a.spec.target ? { target: a.spec.target } : {}) },
    editor: { ...a.editor, nodes: byId(a.editor.nodes) },
  };
  return JSON.stringify(canonical(sorted), null, 2) + "\n";
}

/** Keys documents had before "appliance" was renamed "module" (ADR 0006). */
const LEGACY_KEYS: Record<string, string> = { appliances: "modules", applianceId: "moduleId", appliance: "module" };

/**
 * Renames the keys of a document (or class) written before the rename, at any depth, so older
 * exports, links, drafts and saved versions still open. A key that exists under both names keeps
 * the new one. Values are never changed.
 */
export function migrateLegacyKeys(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(migrateLegacyKeys);
  if (!value || typeof value !== "object") return value;
  const obj = value as Record<string, unknown>;
  const out: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(obj)) {
    const key = LEGACY_KEYS[k] && !(LEGACY_KEYS[k]! in obj) ? LEGACY_KEYS[k]! : k;
    if (key !== k || !(k in LEGACY_KEYS)) out[key] = migrateLegacyKeys(v);
  }
  return out;
}

export type ParseResult = { ok: true; assembly: ApplicationAssembly } | { ok: false; errors: string[] };

/** Parses untrusted JSON text. Never throws; never returns a partially valid assembly. */
export function parseAssembly(text: string, maxBytes = 2_000_000): ParseResult {
  if (text.length > maxBytes) return { ok: false, errors: [`File is larger than ${Math.round(maxBytes / 1000)} kB.`] };
  let raw: unknown;
  try {
    raw = JSON.parse(text);
  } catch (e) {
    return { ok: false, errors: [`Not valid JSON: ${(e as Error).message}`] };
  }
  const r = ApplicationAssemblySchema.safeParse(migrateLegacyKeys(raw));
  if (!r.success) return { ok: false, errors: r.error.issues.slice(0, 12).map((i) => `${i.path.join(".") || "(root)"}: ${i.message}`) };
  const a = r.data;
  const errors: string[] = [];
  const ids = new Set(a.spec.modules.map((x) => x.id));
  if (ids.size !== a.spec.modules.length) errors.push("spec.modules: duplicate module ids.");
  for (const n of a.editor.nodes) if (!ids.has(n.id)) errors.push(`editor.nodes: layout for unknown module "${n.id}".`);
  for (const x of a.spec.modules) if (!a.editor.nodes.some((n) => n.id === x.id)) errors.push(`editor.nodes: missing layout for module "${x.id}".`);
  return errors.length ? { ok: false, errors } : { ok: true, assembly: a };
}
