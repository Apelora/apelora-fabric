import { describe, expect, it } from "vitest";
import { CATEGORIES, SUBCATEGORIES, type ModuleClass } from "@apelora/assembly-model";
import { MODULE_CLASSES } from "@apelora/module-catalog";
import { OTHER, catalogTree, isOpen, searchCatalog, setAll, subKey } from "./groups";

const custom = (id: string, over: Partial<ModuleClass> = {}): ModuleClass => ({
  id, version: "1.0.0", displayName: id, category: "Custom", summary: "", iconAssetPath: "generic-box", runtimeImage: { ref: null },
  architectures: ["x86_64"], ports: [], resources: { cpu: 1, memoryMiB: 128 }, custom: true, ...over,
});

describe("catalog tree", () => {
  const tree = catalogTree(MODULE_CLASSES);

  it("lists categories in taxonomy order, Custom first and always present, with every built-in class once", () => {
    expect(tree.map((g) => g.cat)).toEqual([...CATEGORIES]);
    expect(tree[0]).toMatchObject({ cat: "Custom", count: 0, subs: [], loose: [] });
    const ids = tree.flatMap((g) => [...g.loose, ...g.subs.flatMap((s) => s.items)].map((c) => c.id));
    expect(ids.sort()).toEqual(MODULE_CLASSES.map((c) => c.id).sort());
    for (const g of tree.slice(1)) {
      expect(g.subs.map((s) => s.name), g.cat).toEqual([...SUBCATEGORIES[g.cat]]);
      expect(g.count).toBe(g.subs.reduce((n, s) => n + s.items.length, 0));
    }
  });

  it("puts a custom class in its chosen category, under Other when its subcategory is unknown", () => {
    const t = catalogTree([...MODULE_CLASSES, custom("custom-a"), custom("custom-b", { category: "Data", subcategory: "Relational" }), custom("custom-c", { category: "Data", subcategory: "Nonsense" })]);
    expect(t[0]!.loose.map((c) => c.id)).toEqual(["custom-a"]);
    const data = t.find((g) => g.cat === "Data")!;
    expect(data.subs.find((s) => s.name === "Relational")!.items.map((c) => c.id)).toContain("custom-b");
    expect(data.subs.at(-1)).toMatchObject({ name: OTHER, key: subKey("Data", OTHER) });
    expect(data.subs.at(-1)!.items.map((c) => c.id)).toEqual(["custom-c"]);
  });
});

describe("search", () => {
  it("finds a module in any group and gives its category path", () => {
    const r = searchCatalog(MODULE_CLASSES, "frr");
    expect(r).toEqual([expect.objectContaining({ path: "Networking › Routing" })]);
    expect(r[0]!.items.map((c) => c.id)).toEqual(["frr"]);
  });
  it("matches category and subcategory names, ignores case and surrounding space", () => {
    const r = searchCatalog(MODULE_CLASSES, "  WIDE-COLUMN ");
    expect(r.map((g) => g.path)).toEqual(["Data › Wide-column & graph"]);
    expect(r[0]!.items.map((c) => c.id)).toEqual(["cassandra", "scylladb", "neo4j"]);
    expect(searchCatalog(MODULE_CLASSES, "")).toEqual([]);
    expect(searchCatalog(MODULE_CLASSES, "zzzz")).toEqual([]);
  });
  it("includes matching custom classes under Custom", () => {
    expect(searchCatalog([...MODULE_CLASSES, custom("custom-zz-probe")], "zz-probe").map((g) => g.path)).toEqual(["Custom"]);
  });
});

describe("open state", () => {
  const tree = catalogTree(MODULE_CLASSES);
  it("opens only the first category and the first subcategory of each by default", () => {
    expect(isOpen({}, tree, "Custom")).toBe(true);
    for (const g of tree.slice(1)) {
      expect(isOpen({}, tree, g.key), g.cat).toBe(false);
      g.subs.forEach((s, i) => expect(isOpen({}, tree, s.key), s.key).toBe(i === 0));
    }
  });
  it("a remembered choice wins over the default; expand and collapse all set both levels", () => {
    expect(isOpen({ Custom: false, Data: true }, tree, "Custom")).toBe(false);
    expect(isOpen({ Custom: false, Data: true }, tree, "Data")).toBe(true);
    const all = setAll(tree, true);
    expect(Object.values(all).every(Boolean)).toBe(true);
    expect(all[subKey("Networking", "Routing")]).toBe(true);
    expect(Object.values(setAll(tree, false)).some(Boolean)).toBe(false);
  });
});
