const modules = import.meta.glob("../assets/logos/*.svg", { eager: true, query: "?url", import: "default" }) as Record<string, string>;

/** Resolves an `iconAssetPath` (file stem) to a bundled logo URL. */
export function logoUrl(icon: string): string | undefined {
  return modules[`../assets/logos/${icon}.svg`];
}
