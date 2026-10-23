import { parseAssembly, parseCatalog, unboundCatalogRefs, ensureNics, type ApplicationAssembly } from "@apelora/assembly-model";
import { MODULE_CLASSES, lookupClass } from "@apelora/module-catalog";

import { useCustomCatalog } from "../catalog/customCatalog";
import { assemblyFile, saveText } from "../export/file";
import { assemblyFromDocument } from "./document";
import type { AppBackend } from "./model";

/** Exports an application's design: its draft, or its latest published version when there is no draft. Returns the file name. */
export async function exportApplication(backend: AppBackend, name: string, latest: number | null | undefined): Promise<string> {
  const draft = await backend.getDraft(name);
  const doc = draft ? draft.document : latest ? await backend.versionDocument(name, latest) : null;
  if (!doc) throw new Error(`${name} has no design to export yet.`);
  const r = assemblyFromDocument(doc);
  if (!r.ok) throw new Error(`The saved design cannot be read: ${r.errors[0] ?? "invalid document"}`);
  const file = assemblyFile(r.assembly);
  saveText(file.name, file.text);
  return file.name;
}

/** Reads a file chosen for import. Imports are untrusted input: it goes through the same validation as every other document. */
export async function readImport(file: File): Promise<{ ok: true; assembly: ApplicationAssembly } | { ok: false; errors: string[] }> {
  if (file.size > 5 * 1024 * 1024) return { ok: false, errors: ["The file is larger than 5 MB."] };
  let text: string;
  try { text = await file.text(); } catch { return { ok: false, errors: ["The file could not be read."] }; }
  const r = parseAssembly(text);
  if (!r.ok) return r;
  useCustomCatalog.getState().addMissing(r.assembly.catalog?.classes ?? []);
  return { ok: true, assembly: ensureNics(r.assembly, lookupClass) };
}

/** Reads a catalog file. A problem adds nothing. A built-in class cannot be replaced. */
export async function readCatalog(file: File): Promise<{ ok: true; added: number; total: number } | { ok: false; errors: string[] }> {
  if (file.size > 5 * 1024 * 1024) return { ok: false, errors: ["The file is larger than 5 MB."] };
  let text: string;
  try { text = await file.text(); } catch { return { ok: false, errors: ["The file could not be read."] }; }
  const r = parseCatalog(text);
  if (!r.ok) return r;
  const builtin = new Set(MODULE_CLASSES.map((c) => c.id));
  const clash = r.classes.filter((c) => builtin.has(c.id));
  if (clash.length) return { ok: false, errors: clash.map((c) => `${c.displayName} (${c.id}) is a built-in module and cannot be replaced.`) };
  const missing = unboundCatalogRefs(r.classes, (name, version) => lookupClass(name, version) !== undefined);
  if (missing.length) return { ok: false, errors: missing };
  const added = useCustomCatalog.getState().addMissing(r.classes);
  return { ok: true, added, total: r.classes.length };
}
