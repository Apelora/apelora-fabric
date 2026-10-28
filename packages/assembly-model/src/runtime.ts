import { trimHyphens } from "./ops";
import { effectiveValue, validateProperty } from "./config";
import type { ModuleClass, ModuleInstance, ApplicationAssembly, Property, Availability, GuestOs, InternetAccess, OciImage, Resources, Readiness, Runtime, SystemImage, Volume, VolumeClass } from "./schema";

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

/** `{setting}` or `{setting:yes|no}` in a command word or an environment value. */
const PLACEHOLDER = /\{([a-z][a-z0-9_-]*)(?::([A-Za-z0-9_.-]*)\|([A-Za-z0-9_.-]*))?\}/g;
/** What a choice may be to appear in a command: it can never become a second word. */
const SAFE_CHOICE = /^[A-Za-z0-9_.-]+$/;
/** Environment names Apelora sets itself. */
const RESERVED_ENVIRONMENT = ["APELORA_", "SETUP_", "SECRET_"];

const placeholders = (text: string) => [...text.matchAll(PLACEHOLDER)].map((m) => ({ id: m[1]!, yes: m[2], no: m[3], onOff: m[2] !== undefined }));

/** Problems with the settings an OCI runtime puts into its command and environment. */
function settingProblems(cls: ModuleClass): string[] {
  const rt = cls.runtime;
  if (rt?.kind !== "incus-oci-container") return [];
  const out: string[] = [];
  const prop = (id: string) => cls.properties?.find((p) => p.id === id);
  for (const word of rt.command ?? []) {
    for (const ph of placeholders(word)) {
      const p = prop(ph.id);
      if (!p) { out.push(`${cls.id}: the command uses {${ph.id}}, which is not a setting`); continue; }
      if (p.type === "bool" ? !ph.onOff : ph.onOff) out.push(`${cls.id}: {${ph.id}} must ${p.type === "bool" ? "say what yes and no become ({id:yes|no})" : "not use the yes|no form"}`);
      if (!["int", "enum", "bool"].includes(p.type)) out.push(`${cls.id}: {${ph.id}} is ${p.type}; only number, choice and on/off settings may go into the command`);
      if (p.default === undefined) out.push(`${cls.id}: {${ph.id}} needs a default, so the command always has its value`);
      if (p.type === "enum" && (p.options ?? []).some((o) => !SAFE_CHOICE.test(o))) out.push(`${cls.id}: a choice of {${ph.id}} has characters a command cannot carry safely`);
    }
  }
  for (const [name, value] of Object.entries(rt.environment ?? {})) {
    if (RESERVED_ENVIRONMENT.some((r) => name.startsWith(r))) out.push(`${cls.id}: environment ${name} is reserved for Apelora`);
    for (const ph of placeholders(value)) {
      const p = prop(ph.id);
      if (!p) out.push(`${cls.id}: environment ${name} uses {${ph.id}}, which is not a setting`);
      else if (p.type === "secret") out.push(`${cls.id}: environment ${name} cannot carry the secret reference {${ph.id}}`);
    }
  }
  return out;
}

/**
 * The settings that change what runs: the one that picks the image, and those the command or
 * the environment carries. Every other setting of a deployable class is kept in the design only;
 * one asked when the application is deployed still reaches the instance as its `SETUP_*` or
 * `SECRET_*` variable (`setupVariable`), which the image may not read.
 */
export function appliedSettings(cls: ModuleClass | undefined): Set<string> {
  const rt = cls?.runtime;
  const out = new Set<string>();
  if (!rt) return out;
  if (rt.imageBy) out.add(rt.imageBy.property);
  if (rt.kind === "incus-oci-container") {
    for (const text of [...(rt.command ?? []), ...Object.values(rt.environment ?? {})]) for (const ph of placeholders(text)) out.add(ph.id);
  }
  return out;
}

/** A command word or environment value with the module's settings filled in, or why not. */
function fill(cls: ModuleClass, text: string, config: ModuleInstance["config"]): { ok: true; text: string } | { ok: false; error: string } {
  let error: string | null = null;
  const filled = text.replace(PLACEHOLDER, (_m, id: string, yes?: string, no?: string) => {
    const p = cls.properties?.find((x) => x.id === id);
    const value = p ? effectiveValue(p, config) : undefined;
    const problem = p ? validateProperty(p, value) : null;
    if (!p || problem) { error ??= `${cls.displayName}: ${problem?.message ?? `no setting ${id}`}`; return ""; }
    if (yes !== undefined) return value === true ? yes : (no ?? "");
    return value === undefined ? "" : String(value);
  });
  return error ? { ok: false, error } : { ok: true, text: filled };
}

/** Incus config for an OCI runtime's command (`oci.entrypoint`) and environment, from the module's settings. */
function settingsConfig(cls: ModuleClass, config: ModuleInstance["config"]): { ok: true; config: Record<string, string> } | { ok: false; error: string } {
  const rt = cls.runtime;
  if (rt?.kind !== "incus-oci-container") return { ok: true, config: {} };
  const out: Record<string, string> = {};
  if (rt.command) {
    const words: string[] = [];
    for (const w of rt.command) {
      const f = fill(cls, w, config);
      if (!f.ok) return f;
      if (f.text !== "") words.push(f.text);
    }
    out["oci.entrypoint"] = words.join(" ");
  }
  for (const [name, value] of Object.entries(rt.environment ?? {})) {
    const f = fill(cls, value, config);
    if (!f.ok) return f;
    if (f.text !== "") out[`environment.${name}`] = f.text;
  }
  return { ok: true, config: out };
}

/** Problems with a class's runtime definition (empty when it is sound or design-only). */
export function runtimeProblems(cls: ModuleClass): string[] {
  const rt = cls.runtime;
  if (!rt) return [];
  if (!rt.image === !rt.imageBy) return [`${cls.id}: runtime needs exactly one of image and imageBy`];
  const settings = settingProblems(cls);
  if (settings.length) return settings;
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
  const settings = settingsConfig(cls, module.config);
  if (!settings.ok) return settings;
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
        ...settings.config,
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
    out[`environment.${setupVariable(prop)}`] = String(value);
  }
  return out;
}

/** The environment variable a deploy-time setting reaches the instance as, e.g. `SETUP_SERVER_NAME`. */
export const setupVariable = (prop: Pick<Property, "id" | "type">): string => `${prop.type === "secret" ? "SECRET" : "SETUP"}_${setupToken(prop.id)}`;

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
