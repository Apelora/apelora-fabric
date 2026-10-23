import { serializeAssembly, serializeCatalog, usedCustomClasses, type ApplicationAssembly, type ModuleClass } from "@apelora/assembly-model";
import { lookupClass } from "@apelora/module-catalog";

/** The file for an assembly: its name, and its text with the custom module classes it uses embedded so it is self-contained. */
export function assemblyFile(a: ApplicationAssembly): { name: string; text: string } {
  const used = usedCustomClasses(a, lookupClass);
  const withClasses = { ...a, ...(used.length ? { catalog: { classes: used } } : { catalog: undefined }) };
  return { name: `${a.metadata.name}.assembly.json`, text: serializeAssembly(withClasses) };
}

/** The file for custom module classes, so they can move to another Studio without an application. */
export function catalogFile(classes: readonly ModuleClass[]): { name: string; text: string } {
  return { name: "custom-modules.catalog.json", text: serializeCatalog(classes) };
}

/** Offers text to the person as a download. */
export function saveText(name: string, text: string, type = "application/json"): void {
  const url = URL.createObjectURL(new Blob([text], { type }));
  const link = document.createElement("a");
  link.href = url;
  link.download = name;
  link.click();
  URL.revokeObjectURL(url);
}
