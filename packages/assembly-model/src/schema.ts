import { z } from "zod";
import {
  API_VERSION,
  ARCHITECTURES,
  ASSEMBLY_KIND,
  CATEGORIES,
  DENSITIES,
  LAYOUT_VERSION,
  PORT_TYPES,
  PROPERTY_TYPES,
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

export const ApplianceClassSchema = z.object({
  id: entityId,
  version: z.string().min(1),
  displayName: z.string().min(1).max(48),
  category: z.enum(CATEGORIES),
  summary: z.string().max(200),
  iconAssetPath: z.string(),
  runtimeImage: z.object({ ref: z.string().nullable() }),
  architectures: z.array(z.enum(ARCHITECTURES)).min(1),
  ports: z.array(PortSchema),
  resources: ResourcesSchema,
  properties: z.array(PropertySchema).max(40).optional(),
  custom: z.boolean().optional(),
});

export const ApplianceInstanceSchema = z.object({
  id: entityId,
  name: z.string().min(1).max(48),
  classRef: z.object({ name: entityId, version: z.string().min(1) }),
  resources: ResourcesSchema,
  ports: z.array(PortSchema),
  /** Overrides of class property defaults; only changed values are stored. */
  config: z.record(z.string(), configValue).optional(),
});

const EndpointSchema = z.object({ applianceId: entityId, portId });

export const ConnectionSchema = z.object({
  id: entityId,
  source: EndpointSchema,
  target: EndpointSchema,
});

export const EditorNodeSchema = z.object({
  id: entityId,
  x: finite,
  y: finite,
  width: finite.pipe(z.number().min(64).max(2000)),
  height: finite.pipe(z.number().min(48).max(2000)),
  custom: z.boolean(),
});

export const ApplicationAssemblySchema = z.object({
  apiVersion: z.literal(API_VERSION),
  kind: z.literal(ASSEMBLY_KIND),
  metadata: z.object({ name: z.string().min(1).max(80), uid: z.string().min(1).max(80) }),
  spec: z.object({
    appliances: z.array(ApplianceInstanceSchema),
    connections: z.array(ConnectionSchema),
  }),
  editor: z.object({
    layoutVersion: z.literal(LAYOUT_VERSION),
    density: z.enum(DENSITIES),
    nodes: z.array(EditorNodeSchema),
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
export type ApplianceInstance = z.infer<typeof ApplianceInstanceSchema>;
export type Connection = z.infer<typeof ConnectionSchema>;
export type Endpoint = Connection["source"];
export type EditorNode = z.infer<typeof EditorNodeSchema>;
export type ApplicationAssembly = z.infer<typeof ApplicationAssemblySchema>;
export type Density = ApplicationAssembly["editor"]["density"];
export type Category = ApplianceClass["category"];
