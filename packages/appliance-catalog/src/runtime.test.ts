import { describe, expect, it } from "vitest";
import type { InstanceSpec } from "@apelora/api-client";
import { ApplianceClassSchema, ApplianceInstanceSchema, applicationName, deploymentAppliances, guestOsOf, instanceRequestFor, randomUid, resourceLimits, runtimeProblems, trimHyphens, setInternet, setReplicas, setVolumes, volumesOf, type ApplianceClass } from "@apelora/assembly-model";
import { APPLIANCE_CLASSES, RUNTIMES, lookupClass, sampleCommerceStack } from "./index";

const get = (id: string): ApplianceClass => {
  const c = lookupClass(id, "1.0.0");
  if (!c) throw new Error(`missing class ${id}`);
  return c;
};
const target = { project: "apelora", name: "web-01" };

describe("guest operating system", () => {
  it("is Linux unless a virtual machine class says Windows, and travels with the deployable appliance", () => {
    const nginx = get("nginx");
    expect(guestOsOf(nginx)).toBe("linux");
    expect(guestOsOf(undefined)).toBe("linux");
    const vm = (guestOs?: "windows" | "linux"): ApplianceClass => ({ ...nginx, runtime: { kind: "incus-vm", image: { server: "https://images.example.test", alias: "win/2025" }, ...(guestOs ? { guestOs } : {}) } });
    expect(guestOsOf(vm())).toBe("linux");
    expect(guestOsOf(vm("linux"))).toBe("linux");
    expect(guestOsOf(vm("windows"))).toBe("windows");
    // Only virtual machines can say so; a container class cannot claim to be Windows.
    expect(ApplianceClassSchema.safeParse({ ...nginx, runtime: { ...nginx.runtime, guestOs: "windows" } }).success).toBe(true);
    expect(guestOsOf({ ...nginx, runtime: { ...nginx.runtime, guestOs: "windows" } } as ApplianceClass)).toBe("linux");
    const a = sampleCommerceStack();
    expect(deploymentAppliances(a, lookupClass, "apelora").deployable.every((d) => d.guestOs === "linux")).toBe(true);
  });
});

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
        // The class's resources, as Incus limits.
        config: { "limits.cpu": "1", "limits.memory": "512MiB" },
      },
    });
    const mainline = instanceRequestFor(nginx, { config: { channel: "mainline" } }, target);
    expect(mainline.ok && mainline.request.source.alias).toContain("@sha256:bc4cad49");
    const debian = instanceRequestFor(get("debian"), { config: { version: "13" } }, { ...target, start: false });
    expect(debian.ok && debian.request).toMatchObject({ kind: "container", start: false, source: { protocol: "simplestreams", alias: "debian/13" } });
  });

  it("an appliance's own resources become Incus limits; a container's fraction of a CPU becomes a quota", () => {
    const nginx = get("nginx");
    const half = instanceRequestFor(nginx, { resources: { cpu: 0.5, memoryMiB: 256 } }, target);
    expect(half.ok && half.request.config).toEqual({ "limits.cpu": "1", "limits.cpu.allowance": "50ms/100ms", "limits.memory": "256MiB" });
    const two = instanceRequestFor(nginx, { resources: { cpu: 2.5, memoryMiB: 1024 } }, target);
    expect(two.ok && two.request.config).toEqual({ "limits.cpu": "3", "limits.cpu.allowance": "250ms/100ms", "limits.memory": "1024MiB" });
    expect(resourceLimits("virtual-machine", { cpu: 1.5, memoryMiB: 2048 })).toEqual({ "limits.cpu": "2", "limits.memory": "2048MiB" });
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

  it("runs replicas only for stateless classes", () => {
    const sample = sampleCommerceStack();
    const nginx = sample.spec.appliances.find((a) => a.classRef.name === "nginx")!;
    const debian = sample.spec.appliances.find((a) => a.classRef.name === "debian");
    let doc = setReplicas(sample, nginx.id, 3);
    if (debian) doc = setReplicas(doc, debian.id, 3);
    const { deployable } = deploymentAppliances(doc, lookupClass, "apelora");
    expect(deployable.find((d) => d.appliance === nginx.id)).toMatchObject({ replicas: 3, availability: "stateless" });
    if (debian) expect(deployable.find((d) => d.appliance === debian.id)).toMatchObject({ replicas: 1, availability: "singleton" });
    // 1 is stored as absent; the count is clamped to 1–9.
    expect(setReplicas(doc, nginx.id, 1).spec.appliances.find((a) => a.id === nginx.id)).not.toHaveProperty("replicas");
    expect(setReplicas(doc, nginx.id, 40).spec.appliances.find((a) => a.id === nginx.id)?.replicas).toBe(9);
  });

  it("carries each appliance's internet access, none unless chosen", () => {
    const sample = sampleCommerceStack();
    const doc = setInternet(sample, "nginx-01", "web");
    const { deployable } = deploymentAppliances(doc, lookupClass, "apelora");
    expect(deployable.find((d) => d.appliance === "nginx-01")?.internet).toBe("web");
    expect(deployable.find((d) => d.appliance === "nginx-02")?.internet).toBe("none");
    expect(setInternet(doc, "nginx-01", "none").spec.appliances.find((a) => a.id === "nginx-01")).not.toHaveProperty("internet");
  });

  it("derives a valid application name from any design name", () => {
    expect(applicationName("Commerce Stack (v2)")).toBe("commerce-stack-v2");
    expect(applicationName("2024 shop")).toBe("app-2024-shop");
    expect(applicationName("a".repeat(60))).toHaveLength(40);
    expect(applicationName("x---")).toBe("x");
    expect(applicationName("--Shop--")).toBe("shop");
    expect(applicationName(`${"a".repeat(39)}-b`)).toBe("a".repeat(39));
    // Hyphens are trimmed with a loop: a long run of separators stays fast.
    const t0 = performance.now();
    expect(applicationName(`${"-".repeat(100_000)}x`)).toBe("x");
    expect(performance.now() - t0).toBeLessThan(500);
  });

  it("random ids come from the cryptographic generator and keep their shape", () => {
    const ids = new Set(Array.from({ length: 200 }, () => randomUid()));
    expect(ids.size).toBe(200);
    for (const id of ids) expect(id).toMatch(/^[0-9a-z]{8}$/);
    expect(randomUid(4)).toMatch(/^[0-9a-z]{4}$/);
    expect(trimHyphens("--a-b--")).toBe("a-b");
    expect(trimHyphens("---")).toBe("");
  });
});

describe("volumes", () => {
  const data = { name: "data", sizeMiB: 2048, path: "/var/lib/data" };
  it("come from the class unless the design sets its own, and travel in the request", () => {
    const debian = get("debian");
    const withDefault = { ...debian, runtime: { ...debian.runtime!, volumes: [data] } } as ApplianceClass;
    expect(volumesOf(withDefault, {})).toEqual([data]);
    expect(volumesOf(withDefault, { volumes: [] })).toEqual([]);
    const r = instanceRequestFor(withDefault, { config: {} }, target);
    expect(r.ok && r.request.volumes).toEqual([{ name: "data", sizeMib: 2048, path: "/var/lib/data" }]);
    const spec: InstanceSpec | undefined = r.ok ? r.request : undefined;
    expect(spec?.volumes?.[0]?.sizeMib).toBe(2048);
    const none = instanceRequestFor(debian, { config: {} }, target);
    expect(none.ok && "volumes" in none.request).toBe(false);
  });

  it("are refused for stateless classes, whose replicas are recreated anywhere", () => {
    const r = instanceRequestFor(get("nginx"), { config: {}, volumes: [data] }, target);
    expect(r.ok).toBe(false);
    expect(!r.ok && r.error).toContain("cannot keep volumes");
  });

  it("need a safe absolute mount path, unique names and paths, and bounded sizes", () => {
    const ok = (volumes: unknown) => ApplianceInstanceSchema.shape.volumes.safeParse(volumes).success;
    expect(ok([data, { name: "logs", sizeMiB: 512, path: "/var/log/app" }])).toBe(true);
    for (const path of ["relative", "/", "/srv/../etc", "/srv/./x", "/./x", "/a\nb"]) expect(ok([{ ...data, path }]), path).toBe(false);
    for (const name of ["Data", "-x", "x-", "a_b", "abcdefghijklmnopq"]) expect(ok([{ ...data, name }]), name).toBe(false);
    expect(ok([{ ...data, sizeMiB: 63 }])).toBe(false);
    expect(ok([data, { ...data, path: "/other" }])).toBe(false);
    expect(ok([data, { ...data, name: "two", path: "/var/lib/data/" }])).toBe(false);
    expect(ok(Array.from({ length: 5 }, (_, i) => ({ ...data, name: `v${i}`, path: `/v${i}` })))).toBe(false);
  });

  it("are set per appliance and go back to the class's when cleared", () => {
    const a = sampleCommerceStack();
    const id = a.spec.appliances[0]!.id;
    const set = setVolumes(a, id, [data]);
    expect(set.spec.appliances[0]!.volumes).toEqual([data]);
    expect("volumes" in setVolumes(set, id, undefined).spec.appliances[0]!).toBe(false);
  });
});

