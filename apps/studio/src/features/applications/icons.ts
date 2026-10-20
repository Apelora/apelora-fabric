import { allClasses, logoUrl, GENERIC_ICONS } from "@apelora/appliance-catalog";

import { categoryColor } from "../editor/categories";

export interface IconChoice {
  key: string;
  label: string;
  url: string | undefined;
  tint: string;
}

const NEUTRAL = "oklch(60% 0.03 260)";

/** Every icon the editor already uses: the catalog's logos (tinted like their category) and the generic set. */
export function iconChoices(): IconChoice[] {
  const seen = new Set<string>();
  const out: IconChoice[] = [];
  for (const k of GENERIC_ICONS) {
    seen.add(k);
    out.push({ key: k, label: k.replace(/^generic-/, "").replace(/-/g, " "), url: logoUrl(k), tint: NEUTRAL });
  }
  for (const c of allClasses()) {
    if (seen.has(c.iconAssetPath) || !logoUrl(c.iconAssetPath)) continue;
    seen.add(c.iconAssetPath);
    out.push({ key: c.iconAssetPath, label: c.displayName, url: logoUrl(c.iconAssetPath), tint: categoryColor(c.category) });
  }
  return out;
}

let cache: Map<string, IconChoice> | null = null;

/** The icon for a key; an unknown key falls back to the generic box so a page never shows a hole. */
export function iconFor(key: string): IconChoice {
  cache ??= new Map(iconChoices().map((c) => [c.key, c]));
  return cache.get(key) ?? cache.get("generic-box") ?? { key: "generic-box", label: "box", url: undefined, tint: NEUTRAL };
}
