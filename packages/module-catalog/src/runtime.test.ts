import { describe, expect, it } from "vitest";
import type { InstanceSpec } from "@apelora/api-client";
import { ModuleClassSchema, ModuleInstanceSchema, appliedSettings, applicationName, deploymentModules, guestOsOf, instanceRequestFor, randomUid, resourceLimits, runtimeProblems, trimHyphens, setInternet, setLiveMigration, setReplicas, readinessOf, ReadinessSchema, restartsElsewhere, setReadiness, setVolumes, volumesOf, type ModuleClass } from "@apelora/assembly-model";
import { MODULE_CLASSES, RUNTIMES, lookupClass, sampleCommerceStack } from "./index";

const get = (id: string): ModuleClass => {
  const c = lookupClass(id, "1.0.0");
  if (!c) throw new Error(`missing class ${id}`);
  return c;
};
const target = { project: "apelora", name: "web-01" };

describe("guest operating system", () => {
  it("is Linux unless a virtual machine class says Windows, and travels with the deployable module", () => {
    const nginx = get("nginx");
    expect(guestOsOf(nginx)).toBe("linux");
    expect(guestOsOf(undefined)).toBe("linux");
    const vm = (guestOs?: "windows" | "linux"): ModuleClass => ({ ...nginx, runtime: { kind: "incus-vm", image: { server: "https://images.example.test", alias: "win/2025" }, ...(guestOs ? { guestOs } : {}) } });
    expect(guestOsOf(vm())).toBe("linux");
    expect(guestOsOf(vm("linux"))).toBe("linux");
    expect(guestOsOf(vm("windows"))).toBe("windows");
    // Only virtual machines can say so; a container class cannot claim to be Windows.
    expect(ModuleClassSchema.safeParse({ ...nginx, runtime: { ...nginx.runtime, guestOs: "windows" } }).success).toBe(true);
    expect(guestOsOf({ ...nginx, runtime: { ...nginx.runtime, guestOs: "windows" } } as ModuleClass)).toBe("linux");
    const a = sampleCommerceStack();
    expect(deploymentModules(a, lookupClass, "apelora").deployable.every((d) => d.guestOs === "linux")).toBe(true);
  });
});

describe("runtimes", () => {
  it("every catalog runtime is sound and attached to an existing class", () => {
    for (const id of Object.keys(RUNTIMES)) expect(get(id).runtime).toEqual(RUNTIMES[id]);
    for (const c of MODULE_CLASSES) expect(runtimeProblems(c)).toEqual([]);
  });

  it("picks the image from the module's settings, pinned by digest for OCI", () => {
    const nginx = get("nginx");
    const stable = instanceRequestFor(nginx, {}, target);
    expect(stable).toEqual({
      ok: true,
      request: {
        ...target, kind: "container", start: true, architectures: ["x86_64", "aarch64"],
        source: { protocol: "oci", server: "https://docker.io", alias: "library/nginx@sha256:eb1c896deef834f4e056829fd6dadac8305501f16cc71ba65fb252c8d0aa168e" },
        // The class's resources, as Incus limits.
        config: { "limits.cpu": "1", "limits.memory": "512MiB", "environment.SETUP_SERVER_NAME": "_", "environment.SECRET_CERT_SECRET": "nginx-tls" },
      },
    });
    const mainline = instanceRequestFor(nginx, { config: { channel: "mainline" } }, target);
    expect(mainline.ok && mainline.request.source.alias).toContain("@sha256:c4d1df1c");
    const debian = instanceRequestFor(get("debian"), { config: { version: "13" } }, { ...target, start: false });
    expect(debian.ok && debian.request).toMatchObject({ kind: "container", start: false, source: { protocol: "simplestreams", alias: "debian/13" } });
  });

  it("settings an OCI runtime declares become its command and environment", () => {
    const valkey = get("valkey");
    const plain = instanceRequestFor(valkey, {}, target);
    expect(plain.ok && plain.request.config?.["oci.entrypoint"]).toBe("docker-entrypoint.sh valkey-server --maxmemory 512mb --maxmemory-policy allkeys-lru --appendonly no");
    const tuned = instanceRequestFor(valkey, { config: { maxmemory: 2048, eviction: "noeviction", "append-only": true } }, target);
    expect(tuned.ok && tuned.request.config?.["oci.entrypoint"]).toBe("docker-entrypoint.sh valkey-server --maxmemory 2048mb --maxmemory-policy noeviction --appendonly yes");
    expect(tuned.ok && tuned.request.volumes).toEqual([{ name: "data", sizeMib: 1024, path: "/data" }]);
    // A value the setting does not allow is refused, never passed on.
    const tooBig = instanceRequestFor(valkey, { config: { maxmemory: 10_000_000 } }, target);
    expect(tooBig.ok).toBe(false);
    // Environment values take any setting except a secret reference.
    const withEnv: ModuleClass = { ...valkey, runtime: { ...valkey.runtime!, kind: "incus-oci-container", environment: { MODE: "evict-{eviction}", PERSIST: "{append-only}" } } as ModuleClass["runtime"] };
    const env = instanceRequestFor(withEnv, {}, target);
    expect(env.ok && env.request.config).toMatchObject({ "environment.MODE": "evict-allkeys-lru", "environment.PERSIST": "false" });
  });

  it("a runtime cannot put free text, unknown settings or secrets where they could change the command", () => {
    const valkey = get("valkey");
    const oci = (extra: object): ModuleClass => ({ ...valkey, runtime: { ...valkey.runtime!, ...extra } as ModuleClass["runtime"] });
    const withProp = (p: object, extra: object): ModuleClass => ({ ...oci(extra), properties: [...(valkey.properties ?? []), p as NonNullable<ModuleClass["properties"]>[number]] });
    expect(runtimeProblems(oci({ command: ["valkey-server", "{nope}"] }))).toEqual([expect.stringContaining("not a setting")]);
    expect(runtimeProblems(withProp({ id: "name", label: "Name", type: "string", required: false, default: "x" }, { command: ["run", "--name={name}"] }))).toEqual([expect.stringContaining("only number, choice and on/off")]);
    expect(runtimeProblems(oci({ command: ["run", "{append-only}"] }))).toEqual([expect.stringContaining("say what yes and no become")]);
    expect(runtimeProblems(withProp({ id: "mode", label: "Mode", type: "enum", required: true, default: "a", options: ["a", "b c"] }, { command: ["run", "{mode}"] }))).toEqual([expect.stringContaining("cannot carry safely")]);
    expect(runtimeProblems(withProp({ id: "level", label: "Level", type: "int", required: false }, { command: ["run", "{level}"] }))).toEqual([expect.stringContaining("needs a default")]);
    expect(runtimeProblems(oci({ environment: { SECRET_X: "y" } }))).toEqual([expect.stringContaining("reserved")]);
    expect(runtimeProblems(oci({ environment: { PASSWORD: "{password-secret}" } }))).toEqual([expect.stringContaining("secret reference")]);
    // The schema already refuses quotes and spaces in a command word.
    expect(ModuleClassSchema.safeParse(oci({ command: ["sh", "-c", "rm -rf /"] })).success).toBe(false);
  });

  it("says which settings change what runs; the others stay in the design", () => {
    // Its server name and certificate reach it as SETUP_/SECRET_ variables, which its image does not read.
    expect([...appliedSettings(get("nginx"))]).toEqual(["channel"]);
    expect([...appliedSettings(get("valkey"))].sort()).toEqual(["append-only", "eviction", "maxmemory"]);
    expect(appliedSettings(get("postgresql")).size).toBe(0);
  });

  it("a module's own resources become Incus limits; a container's fraction of a CPU becomes a quota", () => {
    const nginx = get("nginx");
    const half = instanceRequestFor(nginx, { resources: { cpu: 0.5, memoryMiB: 256 } }, target);
    expect(half.ok && half.request.config).toEqual({ "limits.cpu": "1", "limits.cpu.allowance": "50ms/100ms", "limits.memory": "256MiB", "environment.SETUP_SERVER_NAME": "_", "environment.SECRET_CERT_SECRET": "nginx-tls" });
    const two = instanceRequestFor(nginx, { resources: { cpu: 2.5, memoryMiB: 1024 } }, target);
    expect(two.ok && two.request.config).toEqual({ "limits.cpu": "3", "limits.cpu.allowance": "250ms/100ms", "limits.memory": "1024MiB", "environment.SETUP_SERVER_NAME": "_", "environment.SECRET_CERT_SECRET": "nginx-tls" });
    expect(resourceLimits("virtual-machine", { cpu: 1.5, memoryMiB: 2048 })).toEqual({ "limits.cpu": "2", "limits.memory": "2048MiB" });
  });

  it("a deploy-time name and a secret reference are sent to the instance, and a pasted secret is not", () => {
    const nginx = get("nginx");
    const named = instanceRequestFor(nginx, { config: { "server-name": "shop.example.test", "cert-secret": "shop-tls" } }, target);
    expect(named.ok && named.request.config).toMatchObject({
      "environment.SETUP_SERVER_NAME": "shop.example.test",
      "environment.SECRET_CERT_SECRET": "shop-tls",
    });
    const pasted = instanceRequestFor(nginx, { config: { "cert-secret": "Hunter2!" } }, target);
    expect(pasted.ok && pasted.request.config?.["environment.SECRET_CERT_SECRET"]).toBeUndefined();
    const punctuated = instanceRequestFor({ ...nginx, properties: [...(nginx.properties ?? []), { id: "___a---b___", label: "Name", type: "string", required: false, supply: "deploy", default: "ok" }] }, {}, target);
    expect(punctuated.ok && punctuated.request.config?.["environment.SETUP_A_B"]).toBe("ok");
  });

  it("design-only classes and unknown settings are not deployable", () => {
    expect(instanceRequestFor(get("postgresql"), {}, target)).toMatchObject({ ok: false, error: expect.stringContaining("design-only") });
    expect(instanceRequestFor(get("nginx"), { config: { channel: "nightly" } }, target)).toMatchObject({ ok: false, error: expect.stringContaining("no image") });
  });

  it("a runtime must cover every option of its property and set exactly one image source", () => {
    const debian = get("debian");
    const rt = RUNTIMES.debian!;
    const missing = { ...debian, runtime: { ...rt, imageBy: { property: "version", images: { "13": { server: "https://images.example", alias: "debian/13" } } } } } as ModuleClass;
    expect(runtimeProblems(missing)).toEqual(["debian: no image for version = 12"]);
    const both = { ...debian, runtime: { ...rt, image: { server: "https://images.example", alias: "debian/13" } } } as ModuleClass;
    expect(runtimeProblems(both)).toEqual(["debian: runtime needs exactly one of image and imageBy"]);
    const notEnum = { ...debian, runtime: { ...rt, imageBy: { property: "hostname", images: {} } } } as ModuleClass;
    expect(runtimeProblems(notEnum)[0]).toContain("must name an enum property");
  });

  it("the schema refuses unpinned OCI images and non-https servers", () => {
    const base = get("nginx");
    const oci = (reference: string, registry = "https://docker.io") =>
      ModuleClassSchema.safeParse({ ...base, runtime: { kind: "incus-oci-container", image: { registry, reference } } }).success;
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

  it("splits a design into deployable and design-only modules", () => {
    const sample = sampleCommerceStack();
    const { deployable, designOnly } = deploymentModules(sample, lookupClass, "apelora");
    expect(deployable.length + designOnly.length).toBe(sample.spec.modules.length);
    for (const d of deployable) {
      expect(d.spec.project).toBe("apelora");
      expect(d.spec.name).toBe(d.module);
    }
    expect(deployable.map((d) => d.module)).toContain(sample.spec.modules.find((a) => a.classRef.name === "nginx")?.id);
    expect(designOnly.find((d) => d.reason.includes("design-only"))).toBeTruthy();
  });

  it("runs replicas only for stateless classes", () => {
    const sample = sampleCommerceStack();
    const nginx = sample.spec.modules.find((a) => a.classRef.name === "nginx")!;
    const debian = sample.spec.modules.find((a) => a.classRef.name === "debian");
    let doc = setReplicas(sample, nginx.id, 3);
    if (debian) doc = setReplicas(doc, debian.id, 3);
    const { deployable } = deploymentModules(doc, lookupClass, "apelora");
    expect(deployable.find((d) => d.module === nginx.id)).toMatchObject({ replicas: 3, availability: "stateless" });
    if (debian) expect(deployable.find((d) => d.module === debian.id)).toMatchObject({ replicas: 1, availability: "singleton" });
    // 1 is stored as absent; the count is clamped to 1–9.
    expect(setReplicas(doc, nginx.id, 1).spec.modules.find((a) => a.id === nginx.id)).not.toHaveProperty("replicas");
    expect(setReplicas(doc, nginx.id, 40).spec.modules.find((a) => a.id === nginx.id)?.replicas).toBe(9);
  });

  it("carries each module's internet access, none unless chosen", () => {
    const sample = sampleCommerceStack();
    const doc = setInternet(sample, "nginx-01", "web");
    const { deployable } = deploymentModules(doc, lookupClass, "apelora");
    expect(deployable.find((d) => d.module === "nginx-01")?.internet).toBe("web");
    expect(deployable.find((d) => d.module === "nginx-02")?.internet).toBe("none");
    expect(setInternet(doc, "nginx-01", "none").spec.modules.find((a) => a.id === "nginx-01")).not.toHaveProperty("internet");
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
    const withDefault = { ...debian, runtime: { ...debian.runtime!, volumes: [data] } } as ModuleClass;
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
    const ok = (volumes: unknown) => ModuleInstanceSchema.shape.volumes.safeParse(volumes).success;
    expect(ok([data, { name: "logs", sizeMiB: 512, path: "/var/log/app" }])).toBe(true);
    for (const path of ["relative", "/", "/srv/../etc", "/srv/./x", "/./x", "/a\nb"]) expect(ok([{ ...data, path }]), path).toBe(false);
    for (const name of ["Data", "-x", "x-", "a_b", "abcdefghijklmnopq"]) expect(ok([{ ...data, name }]), name).toBe(false);
    expect(ok([{ ...data, sizeMiB: 63 }])).toBe(false);
    expect(ok([data, { ...data, path: "/other" }])).toBe(false);
    expect(ok([data, { ...data, name: "two", path: "/var/lib/data/" }])).toBe(false);
    expect(ok(Array.from({ length: 5 }, (_, i) => ({ ...data, name: `v${i}`, path: `/v${i}` })))).toBe(false);
  });

  it("are set per module and go back to the class's when cleared", () => {
    const a = sampleCommerceStack();
    const id = a.spec.modules[0]!.id;
    const set = setVolumes(a, id, [data]);
    expect(set.spec.modules[0]!.volumes).toEqual([data]);
    expect("volumes" in setVolumes(set, id, undefined).spec.modules[0]!).toBe(false);
  });
});


describe("live migration", () => {
  it("a VM module that asks for it is created to move while running, also when its server is evacuated", () => {
    const vm = get("linux-base");
    const on = instanceRequestFor(vm, { config: {}, liveMigration: true }, target);
    expect(on.ok && on.request.kind).toBe("virtual-machine");
    expect(on.ok && on.request.config).toMatchObject({ "migration.stateful": "true", "cluster.evacuate": "live-migrate" });
    const off = instanceRequestFor(vm, { config: {} }, target);
    expect(off.ok && off.request.config?.["migration.stateful"]).toBeUndefined();
  });

  it("is refused for containers and for VMs with volumes on their server's own pool", () => {
    const c = instanceRequestFor(get("debian"), { config: {}, liveMigration: true }, target);
    expect(!c.ok && c.error).toContain("only virtual machines");
    const v = instanceRequestFor(get("linux-base"), { config: {}, liveMigration: true, volumes: [{ name: "data", sizeMiB: 1024, path: "/data" }] }, target);
    expect(!v.ok && v.error).toContain("cannot follow it while it runs");
  });

  it("is stored only when on", () => {
    const a = sampleCommerceStack();
    const id = a.spec.modules[0]!.id;
    expect(setLiveMigration(a, id, true).spec.modules[0]!.liveMigration).toBe(true);
    expect("liveMigration" in setLiveMigration(setLiveMigration(a, id, true), id, false).spec.modules[0]!).toBe(false);
  });
});

describe("volume classes", () => {
  const nginx = get("nginx");
  const debian = get("debian");
  const vol = (name: string, cls?: "data-local" | "data-replicated") => ({ name, sizeMiB: 1024, path: `/srv/${name}`, ...(cls ? { class: cls } : {}) });

  it("a module's volumes are all on the shared pool or all on its server, never mixed", () => {
    const parse = (volumes: unknown[]) => ModuleInstanceSchema.shape.volumes.safeParse(volumes);
    expect(parse([vol("a"), vol("b")]).success).toBe(true);
    expect(parse([vol("a", "data-local"), vol("b", "data-local")]).success).toBe(true);
    expect(parse([vol("a", "data-replicated"), vol("b", "data-replicated")]).success).toBe(true);
    const mixed = parse([vol("a", "data-replicated"), vol("b")]);
    expect(mixed.success).toBe(false);
    expect(!mixed.success && mixed.error.issues[0]?.message).toContain("cannot be mixed");
    expect(parse([vol("a", "data-replicated"), vol("b", "data-local")]).success).toBe(false);
    expect(parse([{ ...vol("a"), class: "data-fast" }]).success, "an unknown class").toBe(false);
  });

  it("only a shared class is written to the request, so existing designs send what they always did", () => {
    const send = (volumes: ReturnType<typeof vol>[]) => {
      const r = instanceRequestFor(debian, { volumes }, target);
      return r.ok ? r.request.volumes : r.error;
    };
    expect(send([vol("data")])).toEqual([{ name: "data", sizeMib: 1024, path: "/srv/data" }]);
    expect(send([vol("data", "data-local")])).toEqual([{ name: "data", sizeMib: 1024, path: "/srv/data" }]);
    expect(send([vol("data", "data-replicated")])).toEqual([{ name: "data", sizeMib: 1024, path: "/srv/data", class: "data-replicated" }]);
    expect(instanceRequestFor(nginx, { volumes: [vol("data", "data-replicated")] }, target)).toMatchObject({ ok: false });
  });

  it("a module restarts elsewhere only when it has volumes and every one is shared", () => {
    expect(restartsElsewhere([])).toBe(false);
    expect(restartsElsewhere([vol("a")])).toBe(false);
    expect(restartsElsewhere([vol("a", "data-replicated")])).toBe(true);
    expect(restartsElsewhere([vol("a", "data-replicated"), vol("b", "data-replicated")])).toBe(true);
    expect(restartsElsewhere([vol("a", "data-replicated"), vol("b")])).toBe(false);
  });
});

describe("application readiness check", () => {
  const nginx = get("nginx");
  const check = (command: string[], timeoutSecs?: number) => ReadinessSchema.safeParse({ command, ...(timeoutSecs !== undefined ? { timeoutSecs } : {}) });

  it("is a program and arguments, never a shell, with the control plane's limits", () => {
    expect(check(["pg_isready", "-q"]).success).toBe(true);
    expect(check(["/usr/bin/curl", "-fsS", "http://127.0.0.1/health"], 3).success).toBe(true);
    expect(check([]).success).toBe(false);
    expect(check(Array(8).fill("x")).success).toBe(true);
    expect(check(Array(9).fill("x")).success).toBe(false);
    expect(check(["curl", ""]).success).toBe(false);
    expect(check(["curl", "a".repeat(200)]).success).toBe(true);
    expect(check(["curl", "a".repeat(201)]).success).toBe(false);
    expect(check(["curl", "a\nb"]).success, "a control character").toBe(false);
    for (const bad of ["curl; reboot", "$(id)", "a b", "a|b"]) expect(check([bad]).success, bad).toBe(false);
    for (const shell of ["sh", "bash", "/bin/sh", "/usr/bin/bash", "dash", "busybox"]) {
      const r = check([shell, "-c", "true"]);
      expect(r.success, shell).toBe(false);
      expect(!r.success && r.error.issues[0]?.message).toContain("shell");
    }
    expect(check(["shasum"]).success, "only exact shell names are refused").toBe(true);
    expect(check(["pg_isready"], 0).success).toBe(false);
    expect(check(["pg_isready"], 1).success).toBe(true);
    expect(check(["pg_isready"], 15).success).toBe(true);
    expect(check(["pg_isready"], 16).success).toBe(false);
  });

  it("the module's own check wins over its class's, and the deployable module carries it", () => {
    const own = { command: ["curl", "-f", "http://127.0.0.1/"] };
    const withClass = { ...nginx, runtime: { ...nginx.runtime!, readiness: { command: ["from-class"] } } } as ModuleClass;
    expect(readinessOf(nginx, {})).toBeUndefined();
    expect(readinessOf(withClass, {})).toEqual({ command: ["from-class"] });
    expect(readinessOf(withClass, { readiness: own })).toEqual(own);
    let a = sampleCommerceStack();
    const id = a.spec.modules.find((m) => m.classRef.name === "nginx")!.id;
    a = setReadiness(a, id, own);
    const lookup = (n: string, v: string) => lookupClass(n, v);
    const d = deploymentModules(a, lookup, "apelora").deployable.find((m) => m.module === id)!;
    expect(d.readiness).toEqual(own);
    expect(deploymentModules(sampleCommerceStack(), lookup, "apelora").deployable.every((m) => m.readiness === undefined)).toBe(true);
    // Back to the class's: nothing is stored on the module.
    const cleared = setReadiness(a, id, undefined);
    expect(cleared.spec.modules.find((m) => m.id === id)).not.toHaveProperty("readiness");
  });
});
