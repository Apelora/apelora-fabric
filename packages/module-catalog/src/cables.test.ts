import { describe, expect, it } from "vitest";
import { lookupClass, sampleCommerceStack } from "./index";
import { cableColorOf, disconnect, parseAssembly, pruneCableStyles, serializeAssembly, setCableColor, validateAssembly } from "@apelora/assembly-model";

describe("per-cable colour", () => {
  const a0 = sampleCommerceStack();
  const id = a0.spec.connections[0]!.id;

  it("is stored in the editor layout, not in the application model", () => {
    const a = setCableColor(a0, id, "#22c55e");
    expect(cableColorOf(a, id)).toBe("#22c55e");
    expect(a.spec).toBe(a0.spec);
    expect(a.editor.cables).toEqual([{ id, color: "#22c55e" }]);
  });

  it("can be changed and reset to the protocol colour", () => {
    let a = setCableColor(a0, id, "#22c55e");
    a = setCableColor(a, id, "#f43f5e");
    expect(a.editor.cables).toEqual([{ id, color: "#f43f5e" }]);
    a = setCableColor(a, id, null);
    expect(cableColorOf(a, id)).toBeUndefined();
    expect(a.editor.cables ?? []).toEqual([]);
  });

  it("ignores unknown cables and invalid colours", () => {
    expect(setCableColor(a0, "no-such-cable", "#22c55e")).toBe(a0);
    expect(setCableColor(a0, id, "green")).toBe(a0);
  });

  it("is dropped when its cable is removed", () => {
    const a = pruneCableStyles(disconnect(setCableColor(a0, id, "#22c55e"), [id]));
    expect(a.editor.cables ?? []).toEqual([]);
    const untouched = setCableColor(a0, id, "#22c55e");
    expect(pruneCableStyles(untouched)).toBe(untouched);
  });

  it("survives export and import, and the schema rejects bad colours", () => {
    const a = setCableColor(a0, id, "#3b82f6");
    const back = parseAssembly(serializeAssembly(a));
    expect(back.ok && cableColorOf(back.assembly, id)).toBe("#3b82f6");
    const bad = JSON.parse(serializeAssembly(a));
    bad.editor.cables[0].color = "url(javascript:1)";
    expect(parseAssembly(JSON.stringify(bad)).ok).toBe(false);
    expect(validateAssembly(a, lookupClass).filter((d) => d.severity === "error")).toEqual([]);
  });
});
