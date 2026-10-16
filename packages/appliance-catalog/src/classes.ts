import type { ApplianceClass, Port } from "@apelora/assembly-model";

const p = (id: string, name: string, direction: Port["direction"], type: Port["type"], description: string, opts: Partial<Pick<Port, "optional" | "multiplicity">> = {}): Port => ({
  id, name, direction, type, description, optional: opts.optional ?? false, multiplicity: opts.multiplicity ?? "one",
});

const cls = (
  id: string, displayName: string, category: ApplianceClass["category"], summary: string, icon: string,
  ports: Port[], cpu: number, memoryMiB: number,
): ApplianceClass => ({
  id, version: "1.0.0", displayName, category, summary, iconAssetPath: icon,
  runtimeImage: { ref: null }, architectures: ["x86_64", "aarch64"], ports, resources: { cpu, memoryMiB },
});

/** Original, conceptual manifests written for this project. Short port names double as the on-canvas labels. */
export const APPLIANCE_CLASSES: readonly ApplianceClass[] = [
  cls("gateway", "Gateway", "Network & Security", "Application edge: public entry point and routing to the assembly.", "gateway",
    [p("wan", "wan", "input", "network", "Public network uplink", { optional: true }), p("lan", "lan", "output", "http", "Internal side, towards the load balancer", { multiplicity: "many" })], 1, 256),
  cls("opnsense", "OPNsense", "Network & Security", "Open-source firewall and router appliance.", "opnsense",
    [p("wan", "wan", "input", "network", "Uplink", { optional: true }), p("lan", "lan", "output", "network", "Protected network", { multiplicity: "many" })], 2, 1024),
  cls("haproxy", "Load balancer", "Load Balancing", "TCP/HTTP load balancer fronting a pool of web servers.", "haproxy",
    [p("frontend", "in", "input", "http", "Client traffic"), p("backend", "out", "output", "http", "Pool of servers", { multiplicity: "many" })], 1, 512),
  cls("nginx", "NGINX", "Web & Application", "Web server and reverse proxy.", "nginx",
    [p("http", "in", "input", "http", "HTTP requests"), p("db", "db", "output", "postgres", "PostgreSQL connection", { optional: true })], 1, 512),
  cls("apache", "Apache HTTP", "Web & Application", "Apache HTTP Server.", "apache",
    [p("http", "in", "input", "http", "HTTP requests"), p("db", "db", "output", "mysql", "MySQL-compatible connection", { optional: true })], 1, 512),
  cls("nodejs", "Node.js", "Web & Application", "Node.js application runtime.", "nodedotjs",
    [p("http", "in", "input", "http", "HTTP requests"), p("db", "db", "output", "postgres", "PostgreSQL connection", { optional: true }), p("cache", "cache", "output", "redis", "Redis connection", { optional: true })], 1, 512),
  cls("postgresql", "PostgreSQL", "Databases & Cache", "Relational database.", "postgresql",
    [p("postgres", "sql", "input", "postgres", "Client connections", { multiplicity: "many" }), p("metrics", "mon", "output", "metrics", "Metrics endpoint", { optional: true, multiplicity: "many" })], 2, 2048),
  cls("mariadb", "MariaDB", "Databases & Cache", "MySQL-compatible relational database.", "mariadb",
    [p("mysql", "sql", "input", "mysql", "Client connections", { multiplicity: "many" })], 2, 2048),
  cls("redis", "Redis", "Databases & Cache", "In-memory cache and data store.", "redis",
    [p("redis", "in", "input", "redis", "Client connections", { multiplicity: "many" })], 1, 1024),
  cls("minio", "MinIO", "Storage", "S3-compatible object storage.", "minio",
    [p("s3", "s3", "input", "s3", "S3 API clients", { multiplicity: "many" })], 1, 1024),
  cls("prometheus", "Prometheus", "Monitoring", "Metrics collection and alerting.", "prometheus",
    [p("scrape", "mon", "input", "metrics", "Targets to scrape", { optional: true, multiplicity: "many" }), p("query", "api", "output", "http", "Query API", { optional: true, multiplicity: "many" })], 1, 1024),
  cls("grafana", "Grafana", "Monitoring", "Dashboards and visualization.", "grafana",
    [p("source", "src", "output", "postgres", "Data source", { optional: true }), p("ui", "ui", "input", "http", "Web UI", { optional: true })], 1, 512),
  cls("rabbitmq", "RabbitMQ", "Utilities", "Message broker.", "rabbitmq",
    [p("amqp", "amqp", "input", "amqp", "Producers and consumers", { multiplicity: "many" })], 1, 1024),
  cls("linux-base", "Linux base", "Operating Systems", "Minimal Linux base image for custom appliances.", "linux-base",
    [p("net", "net", "input", "network", "Network", { optional: true, multiplicity: "many" })], 1, 512),
];

const customClasses = new Map<string, ApplianceClass>();

/** Replaces the user-defined (custom) classes known to lookups and validation. */
export function setCustomClasses(list: readonly ApplianceClass[]): void {
  customClasses.clear();
  for (const c of list) customClasses.set(`${c.id}@${c.version}`, { ...c, custom: true });
}

export function allClasses(): ApplianceClass[] {
  return [...APPLIANCE_CLASSES, ...customClasses.values()];
}

const key = (name: string, version: string) => `${name}@${version}`;
const index = new Map(APPLIANCE_CLASSES.map((c) => [key(c.id, c.version), c]));

export const lookupClass = (name: string, version: string): ApplianceClass | undefined => index.get(key(name, version)) ?? customClasses.get(key(name, version));
