import { effectiveValue } from "./config";
import type { ApplianceClass, ApplianceInstance, OciImage, Runtime, SystemImage } from "./schema";

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
  appliance: Pick<ApplianceInstance, "config">,
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
  return {
    ok: true,
    request: {
      project: target.project,
      name: target.name,
      kind: rt.kind === "incus-vm" ? "virtual-machine" : "container",
      source,
      // A fingerprint pins one build of one architecture; the server checks it against the cluster.
      architectures: source.fingerprint ? [] : [...cls.architectures],
      start: target.start ?? true,
    },
  };
}
