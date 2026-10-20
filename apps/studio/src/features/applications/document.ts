import { lookupClass } from "@apelora/appliance-catalog";
import { ensureNics, parseAssembly, serializeAssembly, usedCustomClasses, type ApplicationAssembly } from "@apelora/assembly-model";

import { useCustomCatalog } from "../catalog/customCatalog";

/** The document stored for an application: the assembly, with the custom classes it uses so it is self-contained. */
export function documentOf(a: ApplicationAssembly): Record<string, unknown> {
  const used = usedCustomClasses(a, lookupClass);
  const doc = JSON.parse(serializeAssembly(a)) as Record<string, unknown>;
  if (used.length) doc.catalog = { classes: used };
  else delete doc.catalog;
  return doc;
}

/** Reads a stored document through the same validation as an import. Never throws. */
export function assemblyFromDocument(doc: unknown): { ok: true; assembly: ApplicationAssembly } | { ok: false; errors: string[] } {
  const r = parseAssembly(JSON.stringify(doc));
  if (!r.ok) return r;
  useCustomCatalog.getState().addMissing(r.assembly.catalog?.classes ?? []);
  return { ok: true, assembly: ensureNics(r.assembly, lookupClass) };
}
