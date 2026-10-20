import { describe, expect, it } from "vitest";
import type { InstanceSpec } from "@apelora/api-client";
import { ApplianceClassSchema, applicationName, deploymentAppliances, instanceRequestFor, runtimeProblems, type ApplianceClass } from "@apelora/assembly-model";
import { APPLIANCE_CLASSES, RUNTIMES, lookupClass, sampleCommerceStack } from "./index";

const get = (id: string): ApplianceClass => {
  const c = lookupClass(id, "1.0.0");
  if (!c) throw new Error(`missing class ${id}`);
  return c;
};
const target = { project: "apelora", name: "web-01" };

describe("runtimes", () => {
  it("every catalog runtime is sound and attached to an existing class", () => {
    for (const id of Object.keys(RUNTIMES)) expect(get(id).runtime).toEqual(RUNTIMES[id]);
    for (const c of APPLIANCE_CLASSES) expect(runtimeProblems(c)).toEqual([]);
  });

  it("picks the image from the appliance's settings, pinned by digest for OCI", () => {
    const nginx = get("nginx");
    const stable = instanceRequestFor(nginx, {}, target);
    expect(stable).toEqual({
      ok: true,
      request: {
        ...target, kind: "container", start: true, architectures: ["x86_64", "aarch64"],
        source: { protocol: "oci", server: "https://docker.io", alias: "library/nginx@sha256:eb1c896deef834f4e056829fd6dadac8305501f16cc71ba65fb252c8d0aa168e" },
      },
    });
    const mainline = instanceRequestFor(nginx, { config: { channel: "mainline" } }, target);
    expect(mainline.ok && mainline.request.source.alias).toContain("@sha256:bc4cad49");
    const debian = instanceRequestFor(get("debian"), { config: { version: "13" } }, { ...target, start: false });
    expect(debian.ok && debian.request).toMatchObject({ kind: "container", start: false, source: { protocol: "simplestreams", alias: "debian/13" } });
  });

  it("design-only classes and unknown settings are not deployable", () => {
    expect(instanceRequestFor(get("postgresql"), {}, target)).toMatchObject({ ok: false, error: expect.stringContaining("design-only") });
    expect(instanceRequestFor(get("nginx"), { config: { channel: "nightly" } }, target)).toMatchObject({ ok: false, error: expect.stringContaining("no image") });
  });

  it("a runtime must cover every option of its property and set exactly one image source", () => {
    const debian = get("debian");
    const rt = RUNTIMES.debian!;
    const missing = { ...debian, runtime: { ...rt, imageBy: { property: "version", images: { "13": { server: "https://images.example", alias: "debian/13" } } } } } as ApplianceClass;
    expect(runtimeProblems(missing)).toEqual(["debian: no image for version = 12"]);
    const both = { ...debian, runtime: { ...rt, image: { server: "https://images.example", alias: "debian/13" } } } as ApplianceClass;
    expect(runtimeProblems(both)).toEqual(["debian: runtime needs exactly one of image and imageBy"]);
    const notEnum = { ...debian, runtime: { ...rt, imageBy: { property: "hostname", images: {} } } } as ApplianceClass;
    expect(runtimeProblems(notEnum)[0]).toContain("must name an enum property");
  });

  it("the schema refuses unpinned OCI images and non-https servers", () => {
    const base = get("nginx");
    const oci = (reference: string, registry = "https://docker.io") =>
      ApplianceClassSchema.safeParse({ ...base, runtime: { kind: "incus-oci-container", image: { registry, reference } } }).success;
    expect(oci(`library/nginx@sha256:${"a".repeat(64)}`)).toBe(true);
    expect(oci("library/nginx:1.30")).toBe(false);
    expect(oci(`library/nginx@sha256:${"A".repeat(64)}`)).toBe(false);
    expect(oci(`library/nginx@sha256:${"a".repeat(64)}`, "http://docker.io")).toBe(false);
  });

  it("the request is the API's InstanceSpec", () => {
    const r = instanceRequestFor(get("nginx"), {}, target);
    if (!r.ok) throw new Error(r.error);
    const spec: InstanceSpec = r.request;
    expect(spec.kind).toBe("container");
  });

  it("splits a design into deployable and design-only appliances", () => {
    const sample = sampleCommerceStack();
    const { deployable, designOnly } = deploymentAppliances(sample, lookupClass, "apelora");
    expect(deployable.length + designOnly.length).toBe(sample.spec.appliances.length);
    for (const d of deployable) {
      expect(d.spec.project).toBe("apelora");
      expect(d.spec.name).toBe(d.appliance);
    }
    expect(deployable.map((d) => d.appliance)).toContain(sample.spec.appliances.find((a) => a.classRef.name === "nginx")?.id);
    expect(designOnly.find((d) => d.reason.includes("design-only"))).toBeTruthy();
  });

  it("derives a valid application name from any design name", () => {
    expect(applicationName("Commerce Stack (v2)")).toBe("commerce-stack-v2");
    expect(applicationName("2024 shop")).toBe("app-2024-shop");
    expect(applicationName("a".repeat(60))).toHaveLength(40);
    expect(applicationName("x---")).toBe("x");
  });
});
