import { z } from "zod";
import {
  API_VERSION,
  ARCHITECTURES,
  ASSEMBLY_KIND,
  CATALOG_KIND,
  CATEGORIES,
  DENSITIES,
  LEGACY_CATEGORIES,
  LAYOUT_VERSION,
  PLATFORMS,
  PORT_TYPES,
  LABEL_POSITIONS,
  PROPERTY_TYPES,
  SHAPE_KINDS,
  STROKE_STYLES,
} from "./constants";

// No `eval`: the web applications run under a content security policy without 'unsafe-eval'
// (infra/serve/). Zod would otherwise probe for it with `Function("")`, which the browser reports
// as a violation even though the probe is caught (tests/e2e/web-serving.spec.ts).
z.config({ jitless: true });

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

/** A typed, user-editable configuration field declared by a module class. Secrets hold a reference name, never the secret. */
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
  /**
   * `deploy`: asked when the application is deployed (a name, a public address, a secret
   * reference). Absent means the field is part of the design. A secret is still only a reference.
   */
  supply: z.enum(["design", "deploy"]).optional(),
});

export const ResourcesSchema = z.object({
  cpu: z.number().positive().max(256),
  memoryMiB: z.number().positive().max(4_194_304),
});

/** A minimum or maximum for the whole application. An absent bound is not checked. */
const budgetBound = z.number().positive().max(1_048_576);
export const ResourceBudgetSchema = z.object({
  cpuMin: budgetBound.optional(),
  cpuMax: budgetBound.optional(),
  memoryMinMiB: budgetBound.optional(),
  memoryMaxMiB: budgetBound.optional(),
});
export type ResourceBudget = z.infer<typeof ResourceBudgetSchema>;

/**
 * A persistent volume a module keeps (docs/architecture/STORAGE.md): each of its instances
 * gets its own, on the cluster member it runs on, mounted at `path`. Removing the module keeps
 * the volume; it is deleted on purpose in the Fabric Console. The pool is chosen by the platform.
 */
export const MIN_VOLUME_MIB = 64;
export const MAX_VOLUME_MIB = 4 * 1024 * 1024;
export const MAX_VOLUMES = 4;
/**
 * Where a volume lives. `data-local` (the default, not written to the file) is on its server's own
 * pool. `data-replicated` is on the cluster's shared pool, so the module restarts on another server
 * after its server fails and is fenced; it needs the shared pool to be set up (`APELORA_SHARED_POOL`).
 */
export const VOLUME_CLASSES = ["data-local", "data-replicated"] as const;
export type VolumeClass = (typeof VOLUME_CLASSES)[number];
export const VolumeSchema = z.object({
  name: z.string().regex(/^[a-z](?:[a-z0-9-]{0,14}[a-z0-9])?$/, "lowercase letters, digits and hyphens, at most 16, starting with a letter"),
  sizeMiB: z.number().int().min(MIN_VOLUME_MIB).max(MAX_VOLUME_MIB),
  path: z.string().max(255).regex(/^(?!.*(?:^|\/)\.\.?(?:\/|$))\/[^\0\r\n]+$/, "an absolute path below /, without . or .. segments"),
  /** Absent means `data-local`. */
  class: z.enum(VOLUME_CLASSES).optional(),
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
  // A module restarts on another server only when everything it stores is on the shared pool, so
  // the two classes cannot be mixed (the control plane refuses it too).
  const shared = vs.filter((v) => v.class === "data-replicated").length;
  if (shared > 0 && shared < vs.length) ctx.addIssue({ code: "custom", path: [], message: "volumes of class data-replicated and data-local cannot be mixed: a data-local volume cannot follow the module to another server" });
});

/**
 * An application-level readiness check: a program and its arguments (never a shell line) that
 * runs inside each instance as an unprivileged user and exits 0 when the application is ready, for
 * example `pg_isready -q`. The load balancer's port check cannot tell a hung application from a
 * ready one; this can. The control plane enforces the same limits.
 */
export const MAX_READINESS_ARGS = 8;
export const MAX_READINESS_SECONDS = 15;
const SHELLS = new Set(["sh", "bash", "dash", "ash", "zsh", "ksh", "csh", "tcsh", "fish", "busybox"]);
export const ReadinessSchema = z
  .object({
    command: z.array(z.string().min(1).max(200).refine((a) => [...a].every((c) => c.charCodeAt(0) > 31 && c.charCodeAt(0) !== 127), "no control characters")).min(1).max(MAX_READINESS_ARGS),
    timeoutSecs: z.number().int().min(1).max(MAX_READINESS_SECONDS).optional(),
  })
  .superRefine((r, ctx) => {
    const program = r.command[0] ?? "";
    if (!/^[A-Za-z0-9_./+-]+$/.test(program)) ctx.addIssue({ code: "custom", path: ["command", 0], message: "start with a program name or path (letters, digits and _ . / - +)" });
    else if (SHELLS.has(program.split("/").pop() ?? "")) ctx.addIssue({ code: "custom", path: ["command", 0], message: `a shell (${program}) is not allowed: name the program itself` });
  });
export type Readiness = z.infer<typeof ReadinessSchema>;

export const ModuleInstanceSchema = z.object({
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
  /** A VM that keeps running when it is moved to another server; absent means off. */
  liveMigration: z.boolean().optional(),
  /** Its application-level readiness check; absent means the class's (if any). */
  readiness: ReadinessSchema.optional(),
});

/** Outbound access to the internet, per module. Inside the application only cables count. */
export const INTERNET_ACCESS = ["none", "web", "any"] as const;
export type InternetAccess = (typeof INTERNET_ACCESS)[number];

const EndpointSchema = z.object({ moduleId: entityId, portId });

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

/** A zone/sector drawn behind modules. View-only: it never affects deployment semantics. */
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
  modules: z.array(ModuleInstanceSchema).min(1).max(200),
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
 * How a module survives failures: `stateless` instances are interchangeable (several
 * replicas, a lost one is recreated), a `singleton` runs as one instance, `stateful` keeps data
 * that must not be recreated from scratch. Absent means `singleton`.
 */
export const AVAILABILITIES = ["stateless", "singleton", "stateful"] as const;
export type Availability = (typeof AVAILABILITIES)[number];
export const MAX_REPLICAS = 9;
const availability = z.enum(AVAILABILITIES).optional();

/** The operating system inside a virtual machine module. It decides which sessions are offered (a Linux terminal, or PowerShell and remote desktop); absent means Linux. */
export const GUEST_OS = ["linux", "windows"] as const;
export type GuestOs = (typeof GUEST_OS)[number];

/** Volumes a class's modules get unless the design changes them (e.g. a database's data). */
const volumes = volumeList.optional();

export const RuntimeSchema = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("incus-system-container"), image: SystemImageSchema.optional(), imageBy: imageBy(SystemImageSchema).optional(), availability, volumes, readiness: ReadinessSchema.optional() }),
  z.object({ kind: z.literal("incus-vm"), image: SystemImageSchema.optional(), imageBy: imageBy(SystemImageSchema).optional(), availability, guestOs: z.enum(GUEST_OS).optional(), volumes, readiness: ReadinessSchema.optional() }),
  z.object({ kind: z.literal("incus-oci-container"), image: OciImageSchema.optional(), imageBy: imageBy(OciImageSchema).optional(), availability, volumes, readiness: ReadinessSchema.optional() }),
]);

/**
 * Where a class's metrics come from (docs/architecture/MODULE-PORTS.md, "Metrics"):
 * `native` — the software serves them itself, perhaps after a setting or with a bundled plugin;
 * `exporter` — an add-on (a separate process, an agent inside the runtime or a plugin installed
 * separately) turns the software's own statistics into an endpoint; `agent` — an optional
 * operating-system agent installed in the guest; `platform` — the module runs no process and
 * Incus reports its numbers; `none` — the software serves nothing (Incus still reports the
 * instance's resource use).
 */
export const METRICS_SOURCES = ["native", "exporter", "agent", "platform", "none"] as const;
/**
 * What the endpoint answers: `text` — metrics text a collector reads directly; `json` or
 * `page` — the product's own JSON or status page over HTTP; `protocol` — over the software's
 * own protocol (SNMP, LDAP, FastCGI, MQTT, pushed metrics); `counters` — readable only
 * inside the instance (shared memory, files, commands); `none`.
 */
export const METRICS_FORMATS = ["text", "json", "page", "protocol", "counters", "none"] as const;
export const METRICS_AUTH = ["none", "basic", "token", "certificate", "configurable"] as const;
/** How well the facts were checked against the project's own documentation. */
export const METRICS_CONFIDENCE = ["checked", "partly-checked", "unverified"] as const;

const portNumber = z.number().int().min(1).max(65535);
const urlPath = z.string().max(120).regex(/^\/[^\s]*$/, "an absolute path such as /metrics");

/**
 * How the class's software exposes its own metrics. `port` is what a drawn `mon` connection
 * opens (deny by default), so it is the real port, or the one the class's runtime sets when the
 * software has no default (`portChosen`). `portBy` picks it from an enum property (a version).
 */
export const MetricsSchema = z.object({
  source: z.enum(METRICS_SOURCES),
  port: portNumber.optional(),
  portBy: z.object({ property: z.string().min(1).max(48), ports: z.record(z.string(), portNumber) }).optional(),
  /** The software has no default port: the class picks this one and its runtime must set it. */
  portChosen: z.boolean().optional(),
  /** Absent means TCP. */
  transport: z.enum(["tcp", "udp"]).optional(),
  path: urlPath.optional(),
  format: z.enum(METRICS_FORMATS),
  /** Produced at that place without any setting or added software. */
  onByDefault: z.boolean(),
  /** By default it answers only inside the instance (localhost): a setting must change that. */
  localOnly: z.boolean().optional(),
  /** How to turn it on or reach it, in a sentence or two. */
  enable: z.string().min(1).max(400).optional(),
  auth: z.enum(METRICS_AUTH),
  /** Served on a listener of its own, not on the service port. */
  separateListener: z.boolean(),
  health: z.object({ port: portNumber.optional(), path: urlPath }).optional(),
  /** The few numbers worth watching, in a few words each. */
  signals: z.array(z.string().min(1).max(60)).max(8),
  confidence: z.enum(METRICS_CONFIDENCE),
  note: z.string().min(1).max(500).optional(),
});

export const ModuleClassSchema = z.object({
  id: entityId,
  version: z.string().min(1),
  displayName: z.string().min(1).max(48),
  /** A category renamed by the two-level taxonomy reads as the one that replaced it. */
  category: z.preprocess((v) => (typeof v === "string" && Object.hasOwn(LEGACY_CATEGORIES, v) ? LEGACY_CATEGORIES[v] : v), z.enum(CATEGORIES)),
  /** Where the class sits inside its category (`SUBCATEGORIES`). A display hint: a name the Studio does not know groups under "Other". */
  subcategory: z.string().min(1).max(40).optional(),
  summary: z.string().max(200),
  iconAssetPath: z.string(),
  /** Superseded by `runtime`; kept so documents exported earlier still import. */
  runtimeImage: z.object({ ref: z.string().nullable() }),
  runtime: RuntimeSchema.optional(),
  architectures: z.array(z.enum(ARCHITECTURES)).min(1),
  ports: z.array(PortSchema),
  resources: ResourcesSchema,
  properties: z.array(PropertySchema).max(40).optional(),
  /** How the software exposes its own metrics; absent for classes nobody has described yet. */
  metrics: MetricsSchema.optional(),
  composition: CompositionSchema.optional(),
  custom: z.boolean().optional(),
});

/** A file of custom module classes. The same version and kind rules as an application. */
export const ModuleCatalogSchema = z.object({
  apiVersion: z.literal(API_VERSION),
  kind: z.literal(CATALOG_KIND),
  classes: z.array(ModuleClassSchema).min(1).max(200),
});

export const ApplicationAssemblySchema = z.object({
  apiVersion: z.literal(API_VERSION),
  kind: z.literal(ASSEMBLY_KIND),
  metadata: z.object({ name: z.string().min(1).max(80), uid: z.string().min(1).max(80) }),
  spec: z.object({
    modules: z.array(ModuleInstanceSchema),
    connections: z.array(ConnectionSchema),
    links: z.array(LinkTagSchema).optional(),
    /** Design-time hypervisor target; absent means the default platform. A retired target
     *  (`xen`, offered briefly) reads as the default, so files saved with it still import. */
    target: z.object({ platform: z.preprocess((v) => (v === "xen" ? "incus" : v), z.enum(PLATFORMS)) }).optional(),
    /** Minimum and maximum vCPU and memory for the whole application. Absent means no budget. */
    budget: ResourceBudgetSchema.optional(),
  }),
  editor: z.object({
    layoutVersion: z.literal(LAYOUT_VERSION),
    density: z.enum(DENSITIES),
    nodes: z.array(EditorNodeSchema),
    shapes: z.array(ShapeSchema).max(200).optional(),
    /** Per-cable colour overrides (layout only); cables without an entry use their protocol colour. */
    cables: z.array(CableStyleSchema).max(1000).optional(),
  }),
  /** Definitions of custom module classes used by this assembly, so exports are self-contained. */
  catalog: z.object({ classes: z.array(ModuleClassSchema) }).optional(),
});

export type Port = z.infer<typeof PortSchema>;
export type PortDirection = Port["direction"];
export type PortType = Port["type"];
export type Property = z.infer<typeof PropertySchema>;
export type ConfigValue = z.infer<typeof configValue>;
export type Resources = z.infer<typeof ResourcesSchema>;
export type ModuleClass = z.infer<typeof ModuleClassSchema>;
export type Runtime = z.infer<typeof RuntimeSchema>;
export type Metrics = z.infer<typeof MetricsSchema>;
export type SystemImage = z.infer<typeof SystemImageSchema>;
export type OciImage = z.infer<typeof OciImageSchema>;
export type ModuleInstance = z.infer<typeof ModuleInstanceSchema>;
export type Volume = z.infer<typeof VolumeSchema>;
export type Connection = z.infer<typeof ConnectionSchema>;
export type CableStyle = z.infer<typeof CableStyleSchema>;
export type Composition = z.infer<typeof CompositionSchema>;
export type Endpoint = Connection["source"];
export type LinkTag = z.infer<typeof LinkTagSchema>;
export type Shape = z.infer<typeof ShapeSchema>;
export type ShapeKind = Shape["kind"];
export type EditorNode = z.infer<typeof EditorNodeSchema>;
export type ModuleCatalog = z.infer<typeof ModuleCatalogSchema>;
export type ApplicationAssembly = z.infer<typeof ApplicationAssemblySchema>;
export type Density = ApplicationAssembly["editor"]["density"];
export type Category = ModuleClass["category"];
