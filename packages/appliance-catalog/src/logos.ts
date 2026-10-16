const modules = import.meta.glob("../assets/logos/*.svg", { eager: true, query: "?url", import: "default" }) as Record<string, string>;

/** Resolves an `iconAssetPath` (file stem) to a bundled logo URL. */
export function logoUrl(icon: string): string | undefined {
  return modules[`../assets/logos/${icon}.svg`];
}

/** Original, trademark-free icons offered when designing a custom appliance. */
export const GENERIC_ICONS = ["generic-box", "generic-server", "generic-database", "generic-shield", "generic-queue", "generic-chart", "gateway", "haproxy", "linux-base"] as const;
