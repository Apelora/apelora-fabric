import type { Category } from "@apelora/assembly-model";

/** Muted accent per category (oklch), used on node headers, tiles and section headings. */
const HUES: Record<Category, string> = {
  "Network & Security": "oklch(62% 0.13 25)",
  "Load Balancing": "oklch(74% 0.13 90)",
  "Web & Application": "oklch(60% 0.13 250)",
  "Databases & Cache": "oklch(60% 0.14 355)",
  Storage: "oklch(68% 0.13 60)",
  Monitoring: "oklch(66% 0.10 205)",
  "Operating Systems": "oklch(62% 0.11 150)",
  Utilities: "oklch(60% 0.12 300)",
  Custom: "oklch(60% 0.03 260)",
};
export const categoryColor = (c: Category): string => HUES[c];
