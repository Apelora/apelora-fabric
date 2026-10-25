export const PRODUCT_NAME = "Apelora";
export const API_VERSION = "fabric.apelora/v1alpha1" as const;
export const ASSEMBLY_KIND = "ApplicationAssembly" as const;
export const CATALOG_KIND = "ModuleCatalog" as const;
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
  "dns",
  "bgp",
  "imap",
] as const;

/**
 * The catalog taxonomy: a deliberate, closed list of categories, each split into a few
 * subcategories (the Studio's left menu, the generated catalog reference and the website all
 * read it from here). Custom has no subcategories: it holds the user's own modules.
 */
export const CATEGORIES = [
  "Custom",
  "Networking",
  "Runtimes & OS",
  "Data",
  "Messaging",
  "Observability",
  "Security & identity",
  "Developer tools",
  "Applications",
  "Microsoft Windows",
] as const;

export const SUBCATEGORIES: Readonly<Record<(typeof CATEGORIES)[number], readonly string[]>> = {
  Custom: [],
  Networking: ["Application network", "Routing", "Load balancing", "DNS & discovery", "DHCP & IPAM", "VPN", "Firewalls"],
  "Runtimes & OS": ["Web servers", "Language runtimes", "Linux systems"],
  Data: ["Relational", "Key-value & cache", "Document", "Wide-column & graph", "Search", "Analytics", "Object & file storage"],
  Messaging: ["Brokers & queues", "Streaming"],
  Observability: ["Metrics", "Logs", "Traces", "Dashboards", "Alerting & uptime"],
  "Security & identity": ["Identity providers", "Directories", "Secrets & certificates", "Threat detection"],
  "Developer tools": ["Source control", "Build & release"],
  Applications: ["Collaboration", "Websites & wikis", "Mail", "Business"],
  "Microsoft Windows": ["Windows systems", "Directory & access", "Servers & apps"],
};

/**
 * Category names used before the two-level taxonomy (2026-10). Files saved with them (custom
 * classes in an assembly or a catalog file) still import, into the category that replaced them.
 */
export const LEGACY_CATEGORIES: Readonly<Record<string, (typeof CATEGORIES)[number]>> = {
  "Network & Security": "Networking",
  "Load Balancing": "Networking",
  "Security & Identity": "Security & identity",
  "Web & Application": "Applications",
  "Databases & Cache": "Data",
  "Search & Analytics": "Data",
  Storage: "Data",
  "Messaging & Streaming": "Messaging",
  Monitoring: "Observability",
  "Operating Systems": "Runtimes & OS",
  Utilities: "Custom",
};

export const ARCHITECTURES = ["x86_64", "aarch64"] as const;
export const DENSITIES = ["compact", "comfortable", "expanded"] as const;
export const PROPERTY_TYPES = ["string", "int", "bool", "enum", "secret"] as const;
export const SHAPE_KINDS = ["rect", "ellipse", "rounded", "triangle", "note"] as const;

/** Runtime providers an assembly can be designed for. Incus is the supported one (ADR 0001); the others are planned behind the same adapter boundary. Provider facts live in platforms.ts. */
export const PLATFORMS = ["incus", "vmware"] as const;
export const LABEL_POSITIONS = ["top-left", "top", "center", "bottom"] as const;
export const STROKE_STYLES = ["solid", "dashed", "dotted"] as const;
