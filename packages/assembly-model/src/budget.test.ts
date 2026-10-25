import { describe, expect, it } from "vitest";

import { applicationDemand, budgetIssues, setBudget } from "./budget";
import { addModule, createEmptyAssembly, setReplicas, setResources } from "./ops";
import { parseAssembly, serializeAssembly } from "./serialize";
import type { ModuleClass } from "./schema";
import { validateAssembly } from "./validate";

const stateless = (): ModuleClass => ({
  id: "web",
  version: "1.0.0",
  displayName: "Web",
  category: "Applications",
  summary: "",
  iconAssetPath: "nginx",
  runtimeImage: { ref: null },
  architectures: ["x86_64"],
  ports: [],
  resources: { cpu: 1, memoryMiB: 128 },
  runtime: {
    kind: "incus-oci-container",
    availability: "stateless",
    image: { registry: "https://docker.io", reference: `library/nginx@sha256:${"ab".repeat(32)}` },
  },
});

const design = () => {
  const added = addModule(createEmptyAssembly("shop"), stateless(), { x: 0, y: 0 });
  return setReplicas(setResources(added.assembly, added.id, { cpu: 2, memoryMiB: 512 }), added.id, 2);
};

describe("application resource budget", () => {
  it("counts each replica and refuses a design outside the maximum", () => {
    const lookup = () => stateless();
    const a = design();
    expect(applicationDemand(a, lookup)).toEqual({ cpu: 4, memoryMiB: 1024 });
    expect(budgetIssues(a, lookup)).toEqual([]);
    const over = setBudget(a, { cpuMax: 3 });
    expect(budgetIssues(over, lookup).map((p) => p.message)).toEqual(["The application asks for 4 vCPU, above the maximum of 3."]);
    expect(validateAssembly(over, lookup).some((d) => d.code === "resource-budget")).toBe(true);
    const cleared = setBudget(over, { cpuMax: undefined });
    expect(cleared.spec.budget).toBeUndefined();
  });

  it("round-trips a budget and reports a minimum above its maximum", () => {
    const saved = serializeAssembly(setBudget(design(), { cpuMin: 1, memoryMaxMiB: 2048 }));
    const parsed = parseAssembly(saved);
    expect(parsed.ok && parsed.assembly.spec.budget).toEqual({ cpuMin: 1, memoryMaxMiB: 2048 });
    const backwards = setBudget(design(), { memoryMinMiB: 4096, memoryMaxMiB: 1024 });
    expect(budgetIssues(backwards, () => stateless()).some((p) => p.message.includes("minimum is above"))).toBe(true);
  });

  it("does not require a deploy-time field until deployment, and still rejects a bad override", () => {
    const cls = (): ModuleClass => ({
      ...stateless(),
      properties: [{ id: "public-name", label: "Public name", type: "string", required: true, supply: "deploy" }],
    });
    const a = design();
    expect(validateAssembly(a, cls).some((d) => d.code === "config-invalid")).toBe(false);
    const web = a.spec.modules[0];
    if (!web) throw new Error("missing module");
    const invalid = { ...a, spec: { ...a.spec, modules: [{ ...web, config: { "public-name": "x".repeat(300) } }] } };
    expect(validateAssembly(invalid, cls).some((d) => d.code === "config-invalid")).toBe(true);
  });
});
