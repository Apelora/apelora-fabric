import { trimHyphens } from "./ops";
import { effectiveValue, validateProperty } from "./config";
import type { ModuleClass, ModuleInstance, ApplicationAssembly, Availability, GuestOs, InternetAccess, OciImage, Resources, Readiness, Runtime, SystemImage, Volume, VolumeClass } from "./schema";

/**
 * Body of the control plane's "create instance" request (`InstanceSpec` in the API). Kept
 * structurally identical to the generated API type; a test in the catalog checks that.
 */
export interface InstanceRequest {
  project: string;
  name: string;
  kind: "container" | "virtual-machine";
  source: { protocol: "simplestreams" | "oci"; server: string; alias: string; fingerprint?: string };
  architectures: string[];
  start: boolean;
  /** Incus instance options; Apelora sets the resource limits here. */
  config?: Record<string, string>;
  /** Persistent volumes (filesystems); the control plane picks their pool. */
  volumes?: { name: string; sizeMib: number; path?: string; class?: VolumeClass }[];
}

/**
 * Incus limits for a module's resources: memory in MiB, whole CPUs as `limits.cpu`. A
 * container's fractional CPU becomes a hard time quota as well (`0.5` → `50ms/100ms` of one
 * CPU); a VM gets whole vCPUs (rounded up), which is what Incus offers it.
 */
export function resourceLimits(kind: InstanceRequest["kind"], r: Resources): Record<string, string> {
  const limits: Record<string, string> = {
    "limits.cpu": String(Math.max(1, Math.ceil(r.cpu))),
    "limits.memory": `${Math.max(1, Math.round(r.memoryMiB))}MiB`,
  };
  if (kind === "container" && !Number.isInteger(r.cpu)) limits["limits.cpu.allowance"] = `${Math.max(1, Math.round(r.cpu * 100))}ms/100ms`;
  return limits;
}

/** Problems with a class's runtime definition (empty when it is sound or design-only). */
export function runtimeProblems(cls: ModuleClass): string[] {
  const rt = cls.runtime;
  if (!rt) return [];
  if (!rt.image === !rt.imageBy) return [`${cls.id}: runtime needs exactly one of image and imageBy`];
  if (!rt.imageBy) return [];
  const prop = cls.properties?.find((p) => p.id === rt.imageBy?.property);
  if (!prop || prop.type !== "enum" || !prop.options?.length) {
    return [`${cls.id}: imageBy.property ${rt.imageBy.property} must name an enum property`];
  }
  const images = rt.imageBy.images;
  return [
    ...prop.options.filter((o) => !(o in images)).map((o) => `${cls.id}: no image for ${prop.id} = ${o}`),
    ...Object.keys(images).filter((k) => !prop.options?.includes(k)).map((k) => `${cls.id}: image for unknown ${prop.id} = ${k}`),
  ];
}

/** The image a module uses, given its configuration. */
export function runtimeImage(cls: ModuleClass, config: ModuleInstance["config"]): SystemImage | OciImage | undefined {
  const rt = cls.runtime;
  if (!rt) return undefined;
  if (rt.image) return rt.image;
  const prop = cls.properties?.find((p) => p.id === rt.imageBy?.property);
  const value = prop ? effectiveValue(prop, config) : undefined;
  return value === undefined ? undefined : rt.imageBy?.images[String(value)];
}

const isOci = (rt: Runtime, _image: SystemImage | OciImage): _image is OciImage => rt.kind === "incus-oci-container";

/**
 * The create-instance request for one module, or why it cannot be deployed. Pure: no
 * placement, names or project policy beyond what the caller passes in.
 */
export function instanceRequestFor(
  cls: ModuleClass,
  module: Pick<ModuleInstance, "config"> & { resources?: Resources; volumes?: Volume[]; liveMigration?: boolean },
  target: { project: string; name: string; start?: boolean },
): { ok: true; request: InstanceRequest } | { ok: false; error: string } {
  const rt = cls.runtime;
  if (!rt) return { ok: false, error: `${cls.displayName} is design-only: its class has no runtime` };
  const problems = runtimeProblems(cls);
  if (problems.length) return { ok: false, error: problems.join("; ") };
  const image = runtimeImage(cls, module.config);
  if (!image) return { ok: false, error: `${cls.displayName} has no image for its current settings` };
  const source: InstanceRequest["source"] = isOci(rt, image)
    ? { protocol: "oci", server: image.registry, alias: image.reference }
    : { protocol: "simplestreams", server: image.server, alias: image.alias, ...(image.fingerprint ? { fingerprint: image.fingerprint } : {}) };
  const kind = rt.kind === "incus-vm" ? "virtual-machine" : "container";
  const volumes = volumesOf(cls, module);
  if (volumes.length && availabilityOf(cls) === "stateless") {
    return { ok: false, error: `${cls.displayName} is stateless: its replicas cannot keep volumes` };
  }
  if (module.liveMigration && kind !== "virtual-machine") {
    return { ok: false, error: `${cls.displayName} is a container: only virtual machines keep running while they move` };
  }
  if (module.liveMigration && volumes.length) {
    return { ok: false, error: `${cls.displayName} keeps volumes on its server's own pool, which cannot follow it while it runs: turn live migration off or remove the volumes` };
  }
  return {
    ok: true,
    request: {
      project: target.project,
      name: target.name,
      kind,
      source,
      // A fingerprint pins one build of one architecture; the server checks it against the cluster.
      architectures: source.fingerprint ? [] : [...cls.architectures],
      start: target.start ?? true,
      // The module's own resources (set in the Studio), else its class's.
      config: {
        ...resourceLimits(kind, module.resources ?? cls.resources),
        // Moves while running, also when its server is evacuated (docs/design/mobility-and-ha.md).
        ...(module.liveMigration ? { "migration.stateful": "true", "cluster.evacuate": "live-migrate" } : {}),
        ...setupEnvironment(cls, module.config),
      },
      ...(volumes.length ? { volumes: volumes.map((v) => ({ name: v.name, sizeMib: v.sizeMiB, path: v.path, ...(v.class === "data-replicated" ? { class: v.class } : {}) })) } : {}),
    },
  };
}

/** A property id as an environment-key token. A loop, so a long run of punctuation cannot make a regex backtrack. */
function setupToken(id: string): string {
  let out = "";
  let gap = false;
  for (const ch of id.toUpperCase()) {
    const alnum = (ch >= "A" && ch <= "Z") || (ch >= "0" && ch <= "9");
    if (!alnum) { gap = out.length > 0; continue; }
    if (gap) out += "_";
    out += ch;
    gap = false;
  }
  return out;
}

/**
 * Deploy-time fields the instance receives. A name or address is `environment.SETUP_*`.
 * A credential is the reference name in `environment.SECRET_*`. The secret itself is never copied.
 * Keys under `environment.APELORA_*` stay reserved for service addresses.
 */
export function setupEnvironment(cls: ModuleClass, config: ModuleInstance["config"]): Record<string, string> {
  const out: Record<string, string> = {};
  for (const prop of cls.properties ?? []) {
    if (prop.supply !== "deploy") continue;
    const value = effectiveValue(prop, config);
    if (value === undefined || value === "" || validateProperty(prop, value)) continue;
    const token = setupToken(prop.id);
    if (!token) continue;
    out[prop.type === "secret" ? `environment.SECRET_${token}` : `environment.SETUP_${token}`] = String(value);
  }
  return out;
}

/** Applies answers from the deploy step onto a copy of the design. Empty answers leave the design unchanged. */
export function withSetup(
  a: ApplicationAssembly,
  answers: Readonly<Record<string, Readonly<Record<string, string>>>>,
): ApplicationAssembly {
  if (Object.keys(answers).length === 0) return a;
  return {
    ...a,
    spec: {
      ...a.spec,
      modules: a.spec.modules.map((m) => {
        const row = answers[m.id];
        if (!row || Object.keys(row).length === 0) return m;
        const config = { ...(m.config ?? {}) };
        for (const [id, raw] of Object.entries(row)) {
          if (raw === "") delete config[id];
          else config[id] = raw;
        }
        const { config: _drop, ...rest } = m;
        void _drop;
        return Object.keys(config).length > 0 ? { ...rest, config } : rest;
      }),
    },
  };
}

/** A module's volumes: its own when the design sets them, else its class's. */
export const volumesOf = (cls: ModuleClass | undefined, module: { volumes?: Volume[] }): Volume[] =>
  module.volumes ?? cls?.runtime?.volumes ?? [];

/** A module's application-level readiness check: its own when the design sets one, else its class's. */
export const readinessOf = (cls: ModuleClass | undefined, module: { readiness?: Readiness }): Readiness | undefined =>
  module.readiness ?? cls?.runtime?.readiness;

/** Whether every volume (at least one) is on the shared pool, so the module restarts elsewhere after its server fails. */
export const restartsElsewhere = (volumes: Volume[]): boolean => volumes.length > 0 && volumes.every((v) => v.class === "data-replicated");

/** How a class's instances survive failures (`singleton` unless its runtime says otherwise). */
export const availabilityOf = (cls: ModuleClass | undefined): Availability => cls?.runtime?.availability ?? "singleton";

/** The operating system inside a class's instances: `linux` unless a virtual machine class says `windows`. */
export const guestOsOf = (cls: ModuleClass | undefined): GuestOs => (cls?.runtime?.kind === "incus-vm" ? cls.runtime.guestOs : undefined) ?? "linux";

/** How many instances a module runs: its replicas for stateless classes, otherwise one. */
export const replicasOf = (cls: ModuleClass | undefined, module: Pick<ModuleInstance, "replicas">): number =>
  availabilityOf(cls) === "stateless" ? (module.replicas ?? 1) : 1;

/** A module the deployment will run, with the request for its instances. */
export interface DeployableModule {
  module: string;
  name: string;
  spec: InstanceRequest;
  replicas: number;
  availability: Availability;
  internet: InternetAccess;
  guestOs: GuestOs;
  /** Runs inside each instance; exit 0 means the application is ready. */
  readiness?: Readiness;
  /** What each instance gets (also in `spec.config` as Incus limits). */
  resources: Resources;
  /** Volumes each instance keeps (also in `spec.volumes`). */
  volumes: Volume[];
  /** Keeps running when it is moved (also in `spec.config`). */
  liveMigration: boolean;
  /** Answers service names inside the application (ADR 0004). Only the resolver class. */
  dnsResolver: boolean;
}

/** A module that stays in the design only, and why. */
export interface DesignOnlyModule {
  module: string;
  name: string;
  reason: string;
}

/**
 * Splits a design into the modules a deployment runs and those it cannot (no runtime yet,
 * unknown class, components). The control plane names, labels and plans the instances.
 */
export function deploymentModules(
  assembly: ApplicationAssembly,
  lookup: (name: string, version: string) => ModuleClass | undefined,
  project: string,
): { deployable: DeployableModule[]; designOnly: DesignOnlyModule[] } {
  const deployable: DeployableModule[] = [];
  const designOnly: DesignOnlyModule[] = [];
  for (const a of assembly.spec.modules) {
    const cls = lookup(a.classRef.name, a.classRef.version);
    const skip = (reason: string) => designOnly.push({ module: a.id, name: a.name, reason });
    if (!cls) { skip(`unknown module class ${a.classRef.name} ${a.classRef.version}`); continue; }
    if (cls.composition) { skip("components are deployed as their parts in a later version"); continue; }
    const r = instanceRequestFor(cls, a, { project, name: a.id });
    if (r.ok) deployable.push({ module: a.id, name: a.name, spec: r.request, replicas: replicasOf(cls, a), availability: availabilityOf(cls), internet: a.internet ?? "none", guestOs: guestOsOf(cls), ...(readinessOf(cls, a) ? { readiness: readinessOf(cls, a) } : {}), resources: a.resources, volumes: volumesOf(cls, a), liveMigration: a.liveMigration ?? false, dnsResolver: cls.id === "resolver" });
    else skip(r.error);
  }
  return { deployable, designOnly };
}

/** An application name for the control plane from a design's name (letters, digits, hyphens). */
export function applicationName(designName: string): string {
  const s = trimHyphens(trimHyphens(designName.toLowerCase().replace(/[^a-z0-9]+/g, "-")).slice(0, 40));
  return /^[a-z]/.test(s) ? s : trimHyphens(`app-${s}`.slice(0, 40));
}
