import { effectiveValue } from "./config";
import type { ApplianceClass, ApplianceInstance, ApplicationAssembly, Availability, GuestOs, InternetAccess, OciImage, Resources, Runtime, SystemImage } from "./schema";

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
}

/**
 * Incus limits for an appliance's resources: memory in MiB, whole CPUs as `limits.cpu`. A
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
export function runtimeProblems(cls: ApplianceClass): string[] {
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

/** The image an appliance uses, given its configuration. */
export function runtimeImage(cls: ApplianceClass, config: ApplianceInstance["config"]): SystemImage | OciImage | undefined {
  const rt = cls.runtime;
  if (!rt) return undefined;
  if (rt.image) return rt.image;
  const prop = cls.properties?.find((p) => p.id === rt.imageBy?.property);
  const value = prop ? effectiveValue(prop, config) : undefined;
  return value === undefined ? undefined : rt.imageBy?.images[String(value)];
}

const isOci = (rt: Runtime, _image: SystemImage | OciImage): _image is OciImage => rt.kind === "incus-oci-container";

/**
 * The create-instance request for one appliance, or why it cannot be deployed. Pure: no
 * placement, names or project policy beyond what the caller passes in.
 */
export function instanceRequestFor(
  cls: ApplianceClass,
  appliance: Pick<ApplianceInstance, "config"> & { resources?: Resources },
  target: { project: string; name: string; start?: boolean },
): { ok: true; request: InstanceRequest } | { ok: false; error: string } {
  const rt = cls.runtime;
  if (!rt) return { ok: false, error: `${cls.displayName} is design-only: its class has no runtime` };
  const problems = runtimeProblems(cls);
  if (problems.length) return { ok: false, error: problems.join("; ") };
  const image = runtimeImage(cls, appliance.config);
  if (!image) return { ok: false, error: `${cls.displayName} has no image for its current settings` };
  const source: InstanceRequest["source"] = isOci(rt, image)
    ? { protocol: "oci", server: image.registry, alias: image.reference }
    : { protocol: "simplestreams", server: image.server, alias: image.alias, ...(image.fingerprint ? { fingerprint: image.fingerprint } : {}) };
  const kind = rt.kind === "incus-vm" ? "virtual-machine" : "container";
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
      // The appliance's own resources (set in the Studio), else its class's.
      config: resourceLimits(kind, appliance.resources ?? cls.resources),
    },
  };
}

/** How a class's instances survive failures (`singleton` unless its runtime says otherwise). */
export const availabilityOf = (cls: ApplianceClass | undefined): Availability => cls?.runtime?.availability ?? "singleton";

/** The operating system inside a class's instances: `linux` unless a virtual machine class says `windows`. */
export const guestOsOf = (cls: ApplianceClass | undefined): GuestOs => (cls?.runtime?.kind === "incus-vm" ? cls.runtime.guestOs : undefined) ?? "linux";

/** How many instances an appliance runs: its replicas for stateless classes, otherwise one. */
export const replicasOf = (cls: ApplianceClass | undefined, appliance: Pick<ApplianceInstance, "replicas">): number =>
  availabilityOf(cls) === "stateless" ? (appliance.replicas ?? 1) : 1;

/** An appliance the deployment will run, with the request for its instances. */
export interface DeployableAppliance {
  appliance: string;
  name: string;
  spec: InstanceRequest;
  replicas: number;
  availability: Availability;
  internet: InternetAccess;
  guestOs: GuestOs;
  /** What each instance gets (also in `spec.config` as Incus limits). */
  resources: Resources;
}

/** An appliance that stays in the design only, and why. */
export interface DesignOnlyAppliance {
  appliance: string;
  name: string;
  reason: string;
}

/**
 * Splits a design into the appliances a deployment runs and those it cannot (no runtime yet,
 * unknown class, components). The control plane names, labels and plans the instances.
 */
export function deploymentAppliances(
  assembly: ApplicationAssembly,
  lookup: (name: string, version: string) => ApplianceClass | undefined,
  project: string,
): { deployable: DeployableAppliance[]; designOnly: DesignOnlyAppliance[] } {
  const deployable: DeployableAppliance[] = [];
  const designOnly: DesignOnlyAppliance[] = [];
  for (const a of assembly.spec.appliances) {
    const cls = lookup(a.classRef.name, a.classRef.version);
    const skip = (reason: string) => designOnly.push({ appliance: a.id, name: a.name, reason });
    if (!cls) { skip(`unknown appliance class ${a.classRef.name} ${a.classRef.version}`); continue; }
    if (cls.composition) { skip("components are deployed as their parts in a later version"); continue; }
    const r = instanceRequestFor(cls, a, { project, name: a.id });
    if (r.ok) deployable.push({ appliance: a.id, name: a.name, spec: r.request, replicas: replicasOf(cls, a), availability: availabilityOf(cls), internet: a.internet ?? "none", guestOs: guestOsOf(cls), resources: a.resources });
    else skip(r.error);
  }
  return { deployable, designOnly };
}

/** An application name for the control plane from a design's name (letters, digits, hyphens). */
export function applicationName(designName: string): string {
  const s = designName.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 40).replace(/-+$/, "");
  return /^[a-z]/.test(s) ? s : `app-${s}`.slice(0, 40).replace(/-+$/, "");
}
