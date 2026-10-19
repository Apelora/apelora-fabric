import type { ApplianceClass } from "@apelora/assembly-model";
import { cls, p, prop } from "./factory";
import { EXTRA_CLASSES } from "./classes-extra";

/** Original, conceptual manifests written for this project. Short port names double as the on-canvas labels. */
export const APPLIANCE_CLASSES: readonly ApplianceClass[] = [
  cls("gateway", "Gateway", "Network & Security", "Application edge: public entry point and routing to the assembly.", "gateway",
    [p("wan", "wan", "input", "network", "Public network uplink", { optional: true, multiplicity: "one" }), p("lan", "lan", "output", "http", "Internal side, towards the load balancer", { multiplicity: "many" })], 1, 256, [prop("public-name", "Public DNS name", "string", { default: "app.example.test", group: "Network", description: "Name clients use to reach the application." }), prop("tls", "Terminate TLS", "bool", { default: true, group: "Network" })]),
  cls("opnsense", "OPNsense", "Network & Security", "Open-source firewall and router appliance.", "opnsense",
    [p("wan", "wan", "input", "network", "Uplink", { optional: true, multiplicity: "one" }), p("lan", "lan", "output", "network", "Protected network", { multiplicity: "many" })], 2, 1024, [prop("default-policy", "Default policy", "enum", { options: ["deny", "allow"], default: "deny", required: true, group: "Firewall" }), prop("admin-secret", "Admin password reference", "secret", { default: "firewall-admin", required: true, group: "Access", description: "Name of the secret that holds the admin password." })]),
  cls("network", "Network", "Network & Security", "Virtual network segment (no VM). Appliances plug their net interface into it; link it to a gateway or another network through uplink.", "generic-network",
    [p("uplink", "uplink", "input", "network", "Connect this network to another network or to a gateway", { optional: true, multiplicity: "many" }), p("net", "net", "output", "network", "Plug appliance network interfaces (NICs) in here", { multiplicity: "many" })], 1, 64),
  cls("haproxy", "HAProxy", "Load Balancing", "HAProxy: TCP/HTTP load balancer fronting a pool of web servers.", "haproxy",
    [p("frontend", "in", "input", "http", "Client traffic"), p("backend", "out", "output", "http", "Pool of servers", { multiplicity: "many" })], 1, 512, [prop("algorithm", "Balance algorithm", "enum", { options: ["roundrobin", "leastconn", "source"], default: "roundrobin", required: true, group: "Balancing" }), prop("health-path", "Health check path", "string", { default: "/healthz", group: "Balancing" }), prop("max-conn", "Max connections", "int", { default: 4096, min: 16, max: 100000, required: true, group: "Limits" })]),
  cls("nginx", "NGINX", "Web & Application", "Web server and reverse proxy.", "nginx",
    [p("http", "in", "input", "http", "HTTP requests"), p("db", "db", "output", "postgres", "PostgreSQL connection", { optional: true, multiplicity: "one" })], 1, 512, [prop("server-name", "Server name", "string", { default: "_", group: "Server" }), prop("workers", "Worker processes", "int", { default: 2, min: 1, max: 64, required: true, group: "Server" }), prop("gzip", "Gzip responses", "bool", { default: true, group: "Server" })]),
  cls("apache", "Apache httpd", "Web & Application", "Apache HTTP Server (httpd).", "apache",
    [p("http", "in", "input", "http", "HTTP requests"), p("db", "db", "output", "mysql", "MySQL-compatible connection", { optional: true, multiplicity: "one" })], 1, 512, [prop("server-name", "Server name", "string", { default: "localhost", group: "Server" }), prop("mpm", "Processing model", "enum", { options: ["event", "worker", "prefork"], default: "event", required: true, group: "Server" })]),
  cls("nodejs", "Node.js", "Web & Application", "Node.js application runtime.", "nodedotjs",
    [p("http", "in", "input", "http", "HTTP requests"), p("db", "db", "output", "postgres", "PostgreSQL connection", { optional: true, multiplicity: "one" }), p("cache", "cache", "output", "redis", "Redis connection", { optional: true, multiplicity: "one" })], 1, 512, [prop("node-version", "Node.js version", "enum", { options: ["20", "22", "24"], default: "22", required: true, group: "Runtime" }), prop("start-command", "Start command", "string", { default: "npm start", required: true, group: "Runtime" }), prop("node-env", "Environment", "enum", { options: ["production", "development"], default: "production", group: "Runtime" })]),
  cls("postgresql", "PostgreSQL", "Databases & Cache", "Relational database.", "postgresql",
    [p("postgres", "sql", "input", "postgres", "Client connections", { multiplicity: "many" }), p("metrics", "mon", "output", "metrics", "Metrics endpoint", { optional: true, multiplicity: "many" })], 2, 2048, [prop("version", "Version", "enum", { options: ["15", "16", "17"], default: "17", required: true, group: "Database" }), prop("database", "Database name", "string", { default: "app", required: true, group: "Database" }), prop("max-connections", "Max connections", "int", { default: 100, min: 10, max: 5000, required: true, group: "Limits" }), prop("password-secret", "Password secret reference", "secret", { default: "db-password", required: true, group: "Access", description: "Name of the secret that holds the database password." })]),
  cls("mariadb", "MariaDB", "Databases & Cache", "MySQL-compatible relational database.", "mariadb",
    [p("mysql", "sql", "input", "mysql", "Client connections", { multiplicity: "many" })], 2, 2048, [prop("database", "Database name", "string", { default: "app", required: true, group: "Database" }), prop("max-connections", "Max connections", "int", { default: 150, min: 10, max: 5000, required: true, group: "Limits" }), prop("root-secret", "Root password secret reference", "secret", { default: "db-root-password", required: true, group: "Access" })]),
  cls("redis", "Redis", "Databases & Cache", "In-memory cache and data store.", "redis",
    [p("redis", "in", "input", "redis", "Client connections", { multiplicity: "many" })], 1, 1024, [prop("maxmemory", "Max memory (MiB)", "int", { default: 512, min: 16, max: 65536, required: true, group: "Memory" }), prop("eviction", "Eviction policy", "enum", { options: ["noeviction", "allkeys-lru", "volatile-lru"], default: "allkeys-lru", group: "Memory" })]),
  cls("minio", "MinIO", "Storage", "S3-compatible object storage.", "minio",
    [p("s3", "s3", "input", "s3", "S3 API clients", { multiplicity: "many" })], 1, 1024, [prop("bucket", "Default bucket", "string", { default: "data", group: "Storage" }), prop("access-secret", "Access key secret reference", "secret", { default: "s3-access", required: true, group: "Access" })]),
  cls("prometheus", "Prometheus", "Monitoring", "Metrics collection and alerting.", "prometheus",
    [p("scrape", "mon", "input", "metrics", "Targets to scrape", { optional: true, multiplicity: "many" }), p("query", "api", "output", "http", "Query API", { optional: true, multiplicity: "many" })], 1, 1024, [prop("scrape-interval", "Scrape interval (s)", "int", { default: 15, min: 1, max: 3600, required: true, group: "Collection" }), prop("retention-days", "Retention (days)", "int", { default: 15, min: 1, max: 3650, group: "Collection" })]),
  cls("grafana", "Grafana", "Monitoring", "Dashboards and visualization.", "grafana",
    [p("source", "src", "output", "postgres", "Data source", { optional: true }), p("ui", "ui", "input", "http", "Web UI", { optional: true })], 1, 512, [prop("admin-secret", "Admin password secret reference", "secret", { default: "grafana-admin", required: true, group: "Access" }), prop("anonymous", "Allow anonymous viewers", "bool", { default: false, group: "Access" })]),
  cls("rabbitmq", "RabbitMQ", "Messaging & Streaming", "AMQP message broker.", "rabbitmq",
    [p("amqp", "amqp", "input", "amqp", "Producers and consumers", { multiplicity: "many" })], 1, 1024, [prop("vhost", "Virtual host", "string", { default: "/", group: "Broker" }), prop("user-secret", "User password secret reference", "secret", { default: "mq-password", required: true, group: "Access" })]),
  cls("linux-base", "Linux base", "Operating Systems", "Minimal Linux base image for custom appliances.", "linux-base",
    [], 1, 512, [prop("hostname", "Hostname", "string", { default: "node", required: true, group: "System" }), prop("packages", "Extra packages", "string", { group: "System", description: "Space-separated package names to install." })]),
  ...EXTRA_CLASSES,
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
