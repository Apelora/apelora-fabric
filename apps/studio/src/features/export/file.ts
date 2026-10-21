import { serializeAssembly, usedCustomClasses, type ApplicationAssembly } from "@apelora/assembly-model";
import { lookupClass } from "@apelora/module-catalog";

/** The file for an assembly: its name, and its text with the custom module classes it uses embedded so it is self-contained. */
export function assemblyFile(a: ApplicationAssembly): { name: string; text: string } {
  const used = usedCustomClasses(a, lookupClass);
  const withClasses = { ...a, ...(used.length ? { catalog: { classes: used } } : { catalog: undefined }) };
  return { name: `${a.metadata.name}.assembly.json`, text: serializeAssembly(withClasses) };
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
