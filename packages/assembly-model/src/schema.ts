import { z } from "zod";
import {
  API_VERSION,
  ARCHITECTURES,
  ASSEMBLY_KIND,
  CATEGORIES,
  DENSITIES,
  LAYOUT_VERSION,
  PORT_TYPES,
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
  custom: z.boolean().optional(),
});

export const ApplianceInstanceSchema = z.object({
  id: entityId,
  name: z.string().min(1).max(48),
  classRef: z.object({ name: entityId, version: z.string().min(1) }),
  resources: ResourcesSchema,
  ports: z.array(PortSchema),
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
export type Resources = z.infer<typeof ResourcesSchema>;
export type ApplianceClass = z.infer<typeof ApplianceClassSchema>;
export type ApplianceInstance = z.infer<typeof ApplianceInstanceSchema>;
export type Connection = z.infer<typeof ConnectionSchema>;
export type Endpoint = Connection["source"];
export type EditorNode = z.infer<typeof EditorNodeSchema>;
export type ApplicationAssembly = z.infer<typeof ApplicationAssemblySchema>;
export type Density = ApplicationAssembly["editor"]["density"];
export type Category = ApplianceClass["category"];
