import { describe, expect, it } from "vitest";

import { CATEGORIES, LEGACY_CATEGORIES, SUBCATEGORIES } from "./constants";
import { parseCatalog, serializeCatalog, unboundCatalogRefs } from "./serialize";
import { ModuleClassSchema, type ModuleClass } from "./schema";

const box = (id: string, version = "1.0.0"): ModuleClass => ({
  id,
  version,
  displayName: id,
  category: "Custom",
  summary: "",
  iconAssetPath: "generic-box",
  runtimeImage: { ref: null },
  architectures: ["x86_64"],
  ports: [{ id: "net", name: "net", direction: "input", type: "network", optional: true, multiplicity: "one" }],
  resources: { cpu: 1, memoryMiB: 128 },
  custom: true,
});

describe("module catalog file", () => {
  it("round-trips and sorts classes so two exports of the same set match", () => {
    const text = serializeCatalog([box("custom-b"), box("custom-a")]);
    const again = serializeCatalog([box("custom-a"), box("custom-b")]);
    expect(again).toBe(text);
    expect(text.indexOf("custom-a")).toBeLessThan(text.indexOf("custom-b"));
    const parsed = parseCatalog(text);
    expect(parsed.ok && parsed.classes.map((c) => c.id)).toEqual(["custom-a", "custom-b"]);
    expect(parsed.ok && parsed.classes.every((c) => c.custom)).toBe(true);
  });

  it("rejects the whole file when it is not a catalog, repeats a class, or names a missing module", () => {
    expect(parseCatalog("{").ok).toBe(false);
    expect(parseCatalog(JSON.stringify({ apiVersion: "fabric.apelora/v1alpha1", kind: "ApplicationAssembly", classes: [box("custom-a")] })).ok).toBe(false);
    const doubled = JSON.parse(serializeCatalog([box("custom-a")])) as { classes: ModuleClass[] };
    doubled.classes.push(box("custom-a"));
    const bad = parseCatalog(JSON.stringify(doubled));
    expect(bad.ok).toBe(false);
    if (!bad.ok) expect(bad.errors[0]).toContain("appears twice");
    const inner = box("custom-pack");
    inner.composition = {
      modules: [{ id: "web-01", name: "web", classRef: { name: "missing", version: "1.0.0" }, resources: { cpu: 1, memoryMiB: 128 }, ports: [] }],
      connections: [],
      layout: [{ id: "web-01", x: 0, y: 0, width: 128, height: 96, custom: false }],
      exposes: [],
    };
    expect(unboundCatalogRefs([inner], () => false)).toEqual(["custom-pack uses missing, which is not in this catalog."]);
    expect(unboundCatalogRefs([inner], (name) => name === "missing")).toEqual([]);
  });
});

describe("taxonomy", () => {
  it("every category except Custom has a few subcategories, unique within it", () => {
    for (const cat of CATEGORIES) {
      const subs = SUBCATEGORIES[cat];
      expect(new Set(subs).size, cat).toBe(subs.length);
      if (cat === "Custom") expect(subs).toEqual([]);
      else expect(subs.length, cat).toBeGreaterThanOrEqual(2);
      for (const s of subs) expect(s.length, `${cat} › ${s}`).toBeLessThanOrEqual(24);
    }
  });

  it("a file saved with a category name from before the taxonomy still imports, into the category that replaced it", () => {
    for (const [old, now] of Object.entries(LEGACY_CATEGORIES)) {
      expect(CATEGORIES).toContain(now);
      const parsed = parseCatalog(serializeCatalog([{ ...box("custom-old"), category: old as ModuleClass["category"] }]));
      expect(parsed.ok && parsed.classes[0]!.category, old).toBe(now);
    }
    // inherited object keys are not category names
    expect(ModuleClassSchema.safeParse({ ...box("custom-x"), category: "toString" }).success).toBe(false);
    expect(ModuleClassSchema.safeParse({ ...box("custom-x"), category: "Nonsense" }).success).toBe(false);
  });

  it("keeps a subcategory and bounds its length", () => {
    const parsed = parseCatalog(serializeCatalog([{ ...box("custom-sub"), category: "Data", subcategory: "Relational" }]));
    expect(parsed.ok && parsed.classes[0]!.subcategory).toBe("Relational");
    expect(ModuleClassSchema.safeParse({ ...box("custom-x"), subcategory: "x".repeat(41) }).success).toBe(false);
    expect(ModuleClassSchema.safeParse({ ...box("custom-x"), subcategory: "" }).success).toBe(false);
  });
});
