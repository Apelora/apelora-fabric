import type { Category } from "@apelora/assembly-model";

/** Muted accent per category (oklch), used on node headers, tiles and section headings. */
const HUES: Record<Category, string> = {
  Networking: "oklch(62% 0.13 25)",
  "Runtimes & OS": "oklch(62% 0.11 150)",
  Data: "oklch(60% 0.14 355)",
  Messaging: "oklch(62% 0.13 40)",
  Observability: "oklch(66% 0.10 205)",
  "Security & identity": "oklch(58% 0.11 330)",
  "Developer tools": "oklch(60% 0.12 300)",
  Applications: "oklch(60% 0.13 250)",
  "Microsoft Windows": "oklch(60% 0.14 232)",
  Custom: "oklch(60% 0.03 260)",
};
export const categoryColor = (c: Category): string => HUES[c] ?? HUES.Custom;
