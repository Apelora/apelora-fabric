import { describe, expect, it } from "vitest";

import { parseCatalog, serializeCatalog, unboundCatalogRefs } from "./serialize";
import type { ModuleClass } from "./schema";

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
