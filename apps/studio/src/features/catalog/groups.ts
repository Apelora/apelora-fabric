import { CATEGORIES, SUBCATEGORIES, type Category, type ModuleClass } from "@apelora/assembly-model";

/** The group a class without a known subcategory falls into (custom classes, older files). */
export const OTHER = "Other";

export interface SubGroup { name: string; key: string; items: ModuleClass[] }
export interface CategoryGroup { cat: Category; key: string; count: number; subs: SubGroup[]; loose: ModuleClass[] }

/** Open-state keys: a category is its name, a subcategory is `category/subcategory`. */
export const subKey = (cat: Category, sub: string) => `${cat}/${sub}`;

const subOf = (c: ModuleClass): string | undefined => (c.subcategory && SUBCATEGORIES[c.category].includes(c.subcategory) ? c.subcategory : undefined);

/**
 * The catalog as the left menu shows it: categories in taxonomy order, each split into its
 * subcategories (in taxonomy order, tiles in the order given). Classes without a known
 * subcategory go to "Other" at the end of their category; Custom has no subcategories, so its
 * classes are `loose`. Empty subcategories are left out; Custom is always present.
 */
export function catalogTree(classes: readonly ModuleClass[]): CategoryGroup[] {
  return CATEGORIES.map((cat): CategoryGroup => {
    const inCat = classes.filter((c) => c.category === cat);
    if (cat === "Custom") return { cat, key: cat, count: inCat.length, subs: [], loose: inCat };
    const subs: SubGroup[] = SUBCATEGORIES[cat].map((name) => ({ name, key: subKey(cat, name), items: inCat.filter((c) => subOf(c) === name) }));
    const other = inCat.filter((c) => !subOf(c));
    if (other.length) subs.push({ name: OTHER, key: subKey(cat, OTHER), items: other });
    return { cat, key: cat, count: inCat.length, subs: subs.filter((s) => s.items.length > 0), loose: [] };
  }).filter((g) => g.count > 0 || g.cat === "Custom");
}

export interface SearchGroup { cat: Category; sub?: string; path: string; items: ModuleClass[] }

const haystack = (c: ModuleClass) => `${c.displayName} ${c.id} ${c.summary} ${c.category} ${subOf(c) ?? ""}`.toLowerCase();

/** Every class matching the query (name, id, summary or its category path), grouped by path in menu order. */
export function searchCatalog(classes: readonly ModuleClass[], query: string): SearchGroup[] {
  const q = query.trim().toLowerCase();
  if (!q) return [];
  const out: SearchGroup[] = [];
  for (const g of catalogTree(classes)) {
    const groups = g.cat === "Custom" ? [{ sub: undefined as string | undefined, items: g.loose }] : g.subs.map((s) => ({ sub: s.name, items: s.items }));
    for (const { sub, items } of groups) {
      const hits = items.filter((c) => haystack(c).includes(q));
      if (hits.length) out.push({ cat: g.cat, sub, path: sub ? `${g.cat} › ${sub}` : g.cat, items: hits });
    }
  }
  return out;
}

export type OpenState = Readonly<Record<string, boolean>>;

/**
 * Whether a group is open: what the person chose last in this browser, otherwise the default
 * (the first category open, the others closed; the first subcategory of each category open).
 */
export function isOpen(open: OpenState, tree: readonly CategoryGroup[], key: string): boolean {
  const chosen = open[key];
  if (chosen !== undefined) return chosen;
  const catIndex = tree.findIndex((g) => g.key === key);
  if (catIndex >= 0) return catIndex === 0;
  for (const g of tree) if (g.subs.some((s) => s.key === key)) return g.subs[0]?.key === key;
  return false;
}

/** Every key set to one value: expand all or collapse all, at both levels. */
export function setAll(tree: readonly CategoryGroup[], value: boolean): Record<string, boolean> {
  const out: Record<string, boolean> = {};
  for (const g of tree) {
    out[g.key] = value;
    for (const s of g.subs) out[s.key] = value;
  }
  return out;
}
