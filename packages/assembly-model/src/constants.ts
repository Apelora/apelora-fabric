export const PRODUCT_NAME = "Apelora";
export const API_VERSION = "fabric.apelora/v1alpha1" as const;
export const ASSEMBLY_KIND = "ApplicationAssembly" as const;
export const LAYOUT_VERSION = 1 as const;

export const PORT_TYPES = [
  "network",
  "http",
  "tcp",
  "postgres",
  "mysql",
  "redis",
  "amqp",
  "s3",
  "metrics",
  "kafka",
  "pulsar",
  "mqtt",
  "nats",
  "mongodb",
  "cassandra",
  "memcached",
  "grpc",
  "ldap",
  "smtp",
  "syslog",
  "nfs",
  "mssql",
  "smb",
  "rdp",
] as const;

export const CATEGORIES = [
  "Custom",
  "Network & Security",
  "Security & Identity",
  "Load Balancing",
  "Web & Application",
  "Databases & Cache",
  "Messaging & Streaming",
  "Search & Analytics",
  "Storage",
  "Monitoring",
  "Operating Systems",
  "Microsoft Windows",
  "Utilities",
] as const;

export const ARCHITECTURES = ["x86_64", "aarch64"] as const;
export const DENSITIES = ["compact", "comfortable", "expanded"] as const;
export const PROPERTY_TYPES = ["string", "int", "bool", "enum", "secret"] as const;
export const SHAPE_KINDS = ["rect", "ellipse", "rounded", "triangle", "note"] as const;

/** Runtime providers an assembly can be designed for. Incus is the supported one (ADR 0001); the others are planned behind the same adapter boundary. Provider facts live in platforms.ts. */
export const PLATFORMS = ["incus", "xen", "vmware"] as const;
export const LABEL_POSITIONS = ["top-left", "top", "center", "bottom"] as const;
export const STROKE_STYLES = ["solid", "dashed", "dotted"] as const;
