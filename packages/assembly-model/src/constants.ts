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
] as const;

export const CATEGORIES = [
  "Network & Security",
  "Load Balancing",
  "Web & Application",
  "Databases & Cache",
  "Storage",
  "Monitoring",
  "Operating Systems",
  "Utilities",
  "Custom",
] as const;

export const ARCHITECTURES = ["x86_64", "aarch64"] as const;
export const DENSITIES = ["compact", "comfortable", "expanded"] as const;
export const PROPERTY_TYPES = ["string", "int", "bool", "enum", "secret"] as const;
