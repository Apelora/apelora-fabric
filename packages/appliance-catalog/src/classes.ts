import type { ApplianceClass, Port } from "@apelora/assembly-model";

const p = (id: string, name: string, direction: Port["direction"], type: Port["type"], opts: Partial<Pick<Port, "optional" | "multiplicity">> = {}): Port => ({
  id, name, direction, type, optional: opts.optional ?? false, multiplicity: opts.multiplicity ?? "one",
});

const cls = (
  id: string, displayName: string, category: ApplianceClass["category"], summary: string, icon: string,
  ports: Port[], cpu: number, memoryMiB: number,
): ApplianceClass => ({
  id, version: "1.0.0", displayName, category, summary, iconAssetPath: icon,
  runtimeImage: { ref: null }, architectures: ["x86_64", "aarch64"], ports, resources: { cpu, memoryMiB },
});

/** Original, conceptual manifests. They are not derived from any proprietary appliance catalog. */
export const APPLIANCE_CLASSES: readonly ApplianceClass[] = [
  cls("gateway", "Gateway", "Network & Security", "Application edge: public entry point and routing to the assembly.", "gateway",
    [p("wan", "Public network", "input", "network", { optional: true }), p("lan", "Internal", "output", "http", { multiplicity: "many" })], 1, 256),
  cls("opnsense", "OPNsense", "Network & Security", "Open-source firewall and router appliance.", "opnsense",
    [p("wan", "WAN", "input", "network", { optional: true }), p("lan", "LAN", "output", "network", { multiplicity: "many" })], 2, 1024),
  cls("haproxy", "Load balancer", "Load Balancing", "TCP/HTTP load balancer fronting a pool of web servers.", "haproxy",
    [p("frontend", "Frontend", "input", "http"), p("backend", "Backend", "output", "http", { multiplicity: "many" })], 1, 512),
  cls("nginx", "NGINX", "Web & Application", "Web server and reverse proxy.", "nginx",
    [p("http", "HTTP in", "input", "http"), p("db", "Database", "output", "postgres", { optional: true })], 1, 512),
  cls("apache", "Apache HTTP", "Web & Application", "Apache HTTP Server.", "apache",
    [p("http", "HTTP in", "input", "http"), p("db", "Database", "output", "mysql", { optional: true })], 1, 512),
  cls("nodejs", "Node.js", "Web & Application", "Node.js application runtime.", "nodedotjs",
    [p("http", "HTTP in", "input", "http"), p("db", "Database", "output", "postgres", { optional: true }), p("cache", "Cache", "output", "redis", { optional: true })], 1, 512),
  cls("postgresql", "PostgreSQL", "Databases & Cache", "Relational database.", "postgresql",
    [p("postgres", "SQL in", "input", "postgres", { multiplicity: "many" }), p("metrics", "Metrics", "output", "metrics", { optional: true, multiplicity: "many" })], 2, 2048),
  cls("mariadb", "MariaDB", "Databases & Cache", "MySQL-compatible relational database.", "mariadb",
    [p("mysql", "SQL in", "input", "mysql", { multiplicity: "many" })], 2, 2048),
  cls("redis", "Redis", "Databases & Cache", "In-memory cache and data store.", "redis",
    [p("redis", "Client in", "input", "redis", { multiplicity: "many" })], 1, 1024),
  cls("minio", "MinIO", "Storage", "S3-compatible object storage.", "minio",
    [p("s3", "S3 in", "input", "s3", { multiplicity: "many" })], 1, 1024),
  cls("prometheus", "Prometheus", "Monitoring", "Metrics collection and alerting.", "prometheus",
    [p("scrape", "Scrape targets", "input", "metrics", { optional: true, multiplicity: "many" }), p("query", "Query", "output", "http", { optional: true, multiplicity: "many" })], 1, 1024),
  cls("grafana", "Grafana", "Monitoring", "Dashboards and visualization.", "grafana",
    [p("source", "Data source", "output", "postgres", { optional: true }), p("ui", "UI", "input", "http", { optional: true })], 1, 512),
  cls("rabbitmq", "RabbitMQ", "Utilities", "Message broker.", "rabbitmq",
    [p("amqp", "AMQP in", "input", "amqp", { multiplicity: "many" })], 1, 1024),
  cls("linux-base", "Linux base", "Operating Systems", "Minimal Linux base image for custom appliances.", "linux-base",
    [p("net", "Network", "input", "network", { optional: true, multiplicity: "many" })], 1, 512),
];

const key = (name: string, version: string) => `${name}@${version}`;
const index = new Map(APPLIANCE_CLASSES.map((c) => [key(c.id, c.version), c]));

export const lookupClass = (name: string, version: string): ApplianceClass | undefined => index.get(key(name, version));
