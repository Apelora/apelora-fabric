import { z } from "zod";
import {
  API_VERSION,
  ARCHITECTURES,
  ASSEMBLY_KIND,
  CATEGORIES,
  DENSITIES,
  LAYOUT_VERSION,
  PLATFORMS,
  PORT_TYPES,
  LABEL_POSITIONS,
  PROPERTY_TYPES,
  SHAPE_KINDS,
  STROKE_STYLES,
} from "./constants";

const portId = z.string().regex(/^[a-z][a-z0-9-]{0,47}$/, "port id must be kebab-case");
const entityId = z.string().regex(/^[a-z][a-z0-9-]{0,62}$/, "id must be kebab-case");
const finite = z.number().refine(Number.isFinite, "must be a finite number");

export const PortSchema = z.object({
  id: portId,
  name: z.string().min(1).max(48),
  direction: z.enum(["input", "output"]),
  type: z.enum(PORT_TYPES),
  optional: z.boolean(),
  multiplicity: z.enum(["one", "many"]),
  description: z.string().max(160).optional(),
});

const configValue = z.union([z.string().max(512), z.number().refine(Number.isFinite, "must be finite"), z.boolean()]);

/** A typed, user-editable configuration field declared by an appliance class. Secrets hold a reference name, never the secret. */
export const PropertySchema = z.object({
  id: z.string().regex(/^[a-z][a-z0-9_-]{0,47}$/, "property id must be lowercase letters, digits, - or _"),
  label: z.string().min(1).max(48),
  type: z.enum(PROPERTY_TYPES),
  default: configValue.optional(),
  required: z.boolean(),
  description: z.string().max(160).optional(),
  group: z.string().max(32).optional(),
  min: z.number().optional(),
  max: z.number().optional(),
  options: z.array(z.string().min(1).max(48)).max(24).optional(),
});

export const ResourcesSchema = z.object({
  cpu: z.number().positive().max(256),
  memoryMiB: z.number().positive().max(4_194_304),
});

/**
 * A persistent volume an appliance keeps (docs/architecture/STORAGE.md): each of its instances
 * gets its own, on the cluster member it runs on, mounted at `path`. Removing the appliance keeps
 * the volume; it is deleted on purpose in the Fabric Console. The pool is chosen by the platform.
 */
export const MIN_VOLUME_MIB = 64;
export const MAX_VOLUME_MIB = 4 * 1024 * 1024;
export const MAX_VOLUMES = 4;
export const VolumeSchema = z.object({
  name: z.string().regex(/^[a-z](?:[a-z0-9-]{0,14}[a-z0-9])?$/, "lowercase letters, digits and hyphens, at most 16, starting with a letter"),
  sizeMiB: z.number().int().min(MIN_VOLUME_MIB).max(MAX_VOLUME_MIB),
  path: z.string().max(255).regex(/^(?!.*(?:^|\/)\.\.?(?:\/|$))\/[^\0\r\n]+$/, "an absolute path below /, without . or .. segments"),
});
const volumeList = z.array(VolumeSchema).max(MAX_VOLUMES).superRefine((vs, ctx) => {
  const seen = (key: (v: z.infer<typeof VolumeSchema>) => string, what: string) => {
    const s = new Set<string>();
    vs.forEach((v, i) => {
      const k = key(v);
      if (s.has(k)) ctx.addIssue({ code: "custom", path: [i], message: `two volumes have the same ${what} (${k})` });
      s.add(k);
    });
  };
  seen((v) => v.name, "name");
  seen((v) => v.path.replace(/\/+$/, ""), "mount path");
});

export const ApplianceInstanceSchema = z.object({
  id: entityId,
  name: z.string().min(1).max(48),
  classRef: z.object({ name: entityId, version: z.string().min(1) }),
  resources: ResourcesSchema,
  ports: z.array(PortSchema),
  /** Overrides of class property defaults; only changed values are stored. */
  config: z.record(z.string(), configValue).optional(),
  /** Instances to run (1 when absent); more than one only for stateless classes. */
  replicas: z.number().int().min(1).max(9).optional(),
  /** What it may reach outside the application; absent means nothing. */
  internet: z.enum(["none", "web", "any"]).optional(),
  /** Its persistent volumes; absent means the class's (an empty list: none). */
  volumes: volumeList.optional(),
});

/** Outbound access to the internet, per appliance. Inside the application only cables count. */
export const INTERNET_ACCESS = ["none", "web", "any"] as const;
export type InternetAccess = (typeof INTERNET_ACCESS)[number];

const EndpointSchema = z.object({ applianceId: entityId, portId });

export const ConnectionSchema = z.object({
  id: entityId,
  source: EndpointSchema,
  target: EndpointSchema,
});

/** Names a terminal so it is linked, without a drawn cable, to every other terminal with the same name. */
export const LinkTagSchema = z.object({
  id: entityId,
  tag: z.string().regex(/^[a-z0-9][a-z0-9._-]{0,31}$/, "tag must be lowercase letters, digits, . _ or - (max 32)"),
  endpoint: EndpointSchema,
});

const hexColor = z.string().regex(/^#[0-9a-fA-F]{6}$/, "color must be #rrggbb");

/** A zone/sector drawn behind appliances. View-only: it never affects deployment semantics. */
export const CableStyleSchema = z.object({ id: entityId, color: hexColor });

export const ShapeSchema = z.object({
  id: z.string().regex(/^shape-[a-z0-9-]{1,40}$/, "shape id must look like shape-01"),
  kind: z.enum(SHAPE_KINDS),
  x: finite,
  y: finite,
  width: finite.pipe(z.number().min(40).max(6000)),
  height: finite.pipe(z.number().min(30).max(6000)),
  label: z.string().max(60),
  /** Body text of a note (kind "note"); ignored by other shapes. */
  text: z.string().max(600).optional(),
  labelPos: z.enum(LABEL_POSITIONS),
  fontSize: z.number().min(8).max(36),
  fill: hexColor,
  fillOpacity: z.number().min(0).max(1),
  stroke: hexColor,
  strokeWidth: z.number().min(0).max(10),
  strokeStyle: z.enum(STROKE_STYLES),
  radius: z.number().min(0).max(120),
  locked: z.boolean(),
});

export const EditorNodeSchema = z.object({
  id: entityId,
  x: finite,
  y: finite,
  width: finite.pipe(z.number().min(64).max(2000)),
  height: finite.pipe(z.number().min(48).max(2000)),
  custom: z.boolean(),
});

/** A component: an assembly packaged as a class. Its ports map onto inner terminals (the NIC maps onto every inner NIC). */
export const CompositionSchema = z.object({
  appliances: z.array(ApplianceInstanceSchema).min(1).max(200),
  connections: z.array(ConnectionSchema).max(1000),
  links: z.array(LinkTagSchema).max(1000).optional(),
  layout: z.array(EditorNodeSchema).max(200),
  exposes: z.array(z.object({ port: portId, inner: z.array(EndpointSchema).min(1).max(200) })).max(64),
});

const sha256Hex = z.string().regex(/^[0-9a-f]{64}$/, "must be 64 lowercase hex digits");
const httpsServer = z.string().max(200).regex(/^https:\/\/[^\s/?#]+(\/[^\s?#]*)?$/, "must be an https:// URL");

/** A system-container or VM image from a simplestreams image server, optionally pinned to one build. */
export const SystemImageSchema = z.object({
  server: httpsServer,
  alias: z.string().min(1).max(120).regex(/^[A-Za-z0-9][A-Za-z0-9._/-]*$/, "invalid image alias"),
  fingerprint: sha256Hex.optional(),
});

/** An OCI application image, always pinned by digest. */
export const OciImageSchema = z.object({
  registry: httpsServer,
  reference: z.string().max(200).regex(/^[a-z0-9][a-z0-9._/-]*@sha256:[0-9a-f]{64}$/, "pin OCI images by digest: name@sha256:<64 hex digits>"),
  /** Human-readable version of the pinned image, e.g. "1.30.6". */
  version: z.string().max(40).optional(),
});

/** Picks the image from the value of one of the class's enum properties (e.g. its version). */
const imageBy = <T extends z.ZodType>(image: T) =>
  z.object({ property: z.string().min(1).max(48), images: z.record(z.string(), image) });

/**
 * How a class runs on Incus. Exactly one of `image` and `imageBy` is set (checked by
 * `runtimeProblems`). A class without a runtime is design-only: it cannot be deployed.
 */
/**
 * How an appliance survives failures: `stateless` instances are interchangeable (several
 * replicas, a lost one is recreated), a `singleton` runs as one instance, `stateful` keeps data
 * that must not be recreated from scratch. Absent means `singleton`.
 */
export const AVAILABILITIES = ["stateless", "singleton", "stateful"] as const;
export type Availability = (typeof AVAILABILITIES)[number];
export const MAX_REPLICAS = 9;
const availability = z.enum(AVAILABILITIES).optional();

/** The operating system inside a virtual machine appliance. It decides which sessions are offered (a Linux terminal, or PowerShell and remote desktop); absent means Linux. */
export const GUEST_OS = ["linux", "windows"] as const;
export type GuestOs = (typeof GUEST_OS)[number];

/** Volumes a class's appliances get unless the design changes them (e.g. a database's data). */
const volumes = volumeList.optional();

export const RuntimeSchema = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("incus-system-container"), image: SystemImageSchema.optional(), imageBy: imageBy(SystemImageSchema).optional(), availability, volumes }),
  z.object({ kind: z.literal("incus-vm"), image: SystemImageSchema.optional(), imageBy: imageBy(SystemImageSchema).optional(), availability, guestOs: z.enum(GUEST_OS).optional(), volumes }),
  z.object({ kind: z.literal("incus-oci-container"), image: OciImageSchema.optional(), imageBy: imageBy(OciImageSchema).optional(), availability, volumes }),
]);

export const ApplianceClassSchema = z.object({
  id: entityId,
  version: z.string().min(1),
  displayName: z.string().min(1).max(48),
  category: z.enum(CATEGORIES),
  summary: z.string().max(200),
  iconAssetPath: z.string(),
  /** Superseded by `runtime`; kept so documents exported earlier still import. */
  runtimeImage: z.object({ ref: z.string().nullable() }),
  runtime: RuntimeSchema.optional(),
  architectures: z.array(z.enum(ARCHITECTURES)).min(1),
  ports: z.array(PortSchema),
  resources: ResourcesSchema,
  properties: z.array(PropertySchema).max(40).optional(),
  composition: CompositionSchema.optional(),
  custom: z.boolean().optional(),
});

export const ApplicationAssemblySchema = z.object({
  apiVersion: z.literal(API_VERSION),
  kind: z.literal(ASSEMBLY_KIND),
  metadata: z.object({ name: z.string().min(1).max(80), uid: z.string().min(1).max(80) }),
  spec: z.object({
    appliances: z.array(ApplianceInstanceSchema),
    connections: z.array(ConnectionSchema),
    links: z.array(LinkTagSchema).optional(),
    /** Design-time hypervisor target; absent means the default platform. A retired target
     *  (`xen`, offered briefly) reads as the default, so files saved with it still import. */
    target: z.object({ platform: z.preprocess((v) => (v === "xen" ? "incus" : v), z.enum(PLATFORMS)) }).optional(),
  }),
  editor: z.object({
    layoutVersion: z.literal(LAYOUT_VERSION),
    density: z.enum(DENSITIES),
    nodes: z.array(EditorNodeSchema),
    shapes: z.array(ShapeSchema).max(200).optional(),
    /** Per-cable colour overrides (layout only); cables without an entry use their protocol colour. */
    cables: z.array(CableStyleSchema).max(1000).optional(),
  }),
  /** Definitions of custom appliance classes used by this assembly, so exports are self-contained. */
  catalog: z.object({ classes: z.array(ApplianceClassSchema) }).optional(),
});

export type Port = z.infer<typeof PortSchema>;
export type PortDirection = Port["direction"];
export type PortType = Port["type"];
export type Property = z.infer<typeof PropertySchema>;
export type ConfigValue = z.infer<typeof configValue>;
export type Resources = z.infer<typeof ResourcesSchema>;
export type ApplianceClass = z.infer<typeof ApplianceClassSchema>;
export type Runtime = z.infer<typeof RuntimeSchema>;
export type SystemImage = z.infer<typeof SystemImageSchema>;
export type OciImage = z.infer<typeof OciImageSchema>;
export type ApplianceInstance = z.infer<typeof ApplianceInstanceSchema>;
export type Volume = z.infer<typeof VolumeSchema>;
export type Connection = z.infer<typeof ConnectionSchema>;
export type CableStyle = z.infer<typeof CableStyleSchema>;
export type Composition = z.infer<typeof CompositionSchema>;
export type Endpoint = Connection["source"];
export type LinkTag = z.infer<typeof LinkTagSchema>;
export type Shape = z.infer<typeof ShapeSchema>;
export type ShapeKind = Shape["kind"];
export type EditorNode = z.infer<typeof EditorNodeSchema>;
export type ApplicationAssembly = z.infer<typeof ApplicationAssemblySchema>;
export type Density = ApplicationAssembly["editor"]["density"];
export type Category = ApplianceClass["category"];
