import type { ModuleClass } from "@apelora/assembly-model";
import { cls, p, prop } from "./factory";
import { EXTRA_CLASSES } from "./classes-extra";
import { RUNTIMES } from "./runtimes";

/** Original, conceptual manifests written for this project. Short port names double as the on-canvas labels. */
const BASE_CLASSES: readonly ModuleClass[] = [
  cls("gateway", "Gateway", "Network & Security", "Application edge: public entry point and routing to the assembly.", "gateway",
    [p("wan", "wan", "input", "network", "Public network uplink", { optional: true, multiplicity: "one" }), p("lan", "lan", "output", "http", "Internal side, towards the load balancer", { multiplicity: "many" })], 1, 256, [prop("public-name", "Public DNS name", "string", { default: "app.example.test", group: "Network", description: "Name clients use to reach the application." }), prop("tls", "Terminate TLS", "bool", { default: true, group: "Network" })]),
  cls("opnsense", "OPNsense", "Network & Security", "Open-source firewall and router module.", "opnsense",
    [p("wan", "wan", "input", "network", "Uplink", { optional: true, multiplicity: "one" }), p("lan", "lan", "output", "network", "Protected network", { multiplicity: "many" })], 2, 1024, [prop("default-policy", "Default policy", "enum", { options: ["deny", "allow"], default: "deny", required: true, group: "Firewall" }), prop("admin-secret", "Admin password reference", "secret", { default: "firewall-admin", required: true, group: "Access", description: "Name of the secret that holds the admin password." })]),
  cls("network", "Network", "Network & Security", "Virtual network segment; it runs no instance. Modules plug their net interface into it; link it to a gateway or another network through uplink.", "generic-network",
    [p("uplink", "uplink", "input", "network", "Connect this network to another network or to a gateway", { optional: true, multiplicity: "many" }), p("net", "net", "output", "network", "Plug module network interfaces (NICs) in here", { multiplicity: "many" })], 1, 64),
  cls("haproxy", "HAProxy", "Load Balancing", "HAProxy: TCP/HTTP load balancer and reverse proxy fronting a pool of servers, with health checks and high availability.", "haproxy",
    [p("frontend", "in", "input", "http", "Client traffic"), p("backend", "out", "output", "http", "Pool of servers", { multiplicity: "many" }), p("mon", "mon", "output", "metrics", "Metrics endpoint for Prometheus-style scraping", { optional: true, multiplicity: "many" })], 1, 512, [
      prop("algorithm", "Balance algorithm", "enum", { options: ["roundrobin", "leastconn", "source"], default: "roundrobin", required: true, group: "Balancing" }),
      prop("sticky", "Sticky sessions", "bool", { default: false, group: "Balancing", description: "Keep each client on the same server." }),
      prop("mode", "Proxy mode", "enum", { options: ["http", "tcp"], default: "http", required: true, group: "Proxy", description: "http inspects requests (layer 7); tcp forwards connections as-is (layer 4)." }),
      prop("health-path", "Health check path", "string", { default: "/healthz", group: "Health checks" }),
      prop("health-interval-s", "Check interval (s)", "int", { default: 2, min: 1, max: 300, required: true, group: "Health checks" }),
      prop("max-conn", "Max connections", "int", { default: 4096, min: 16, max: 100000, required: true, group: "Limits" }),
      prop("client-timeout-s", "Client timeout (s)", "int", { default: 30, min: 1, max: 3600, required: true, group: "Limits" }),
      prop("server-timeout-s", "Server timeout (s)", "int", { default: 30, min: 1, max: 3600, required: true, group: "Limits" }),
      prop("tls", "Terminate TLS", "bool", { default: false, group: "TLS" }),
      prop("cert-secret", "Certificate secret reference", "secret", { default: "haproxy-tls", group: "TLS", description: "Name of the secret that holds the certificate and private key." }),
      prop("compress", "Compress responses", "bool", { default: false, group: "Proxy" }),
      prop("stats", "Statistics page", "bool", { default: false, group: "Admin" }),
      prop("channel", "Release branch", "enum", { options: ["lts", "latest"], default: "lts", required: true, group: "Software", description: "Long-term-support branches get fixes for years; the latest branch is for early adopters." }),
    ]),
  cls("nginx", "NGINX", "Web & Application", "Web server, reverse proxy, load balancer and content cache.", "nginx",
    [p("http", "in", "input", "http", "HTTP requests"), p("db", "db", "output", "postgres", "PostgreSQL connection", { optional: true, multiplicity: "one" }), p("upstream", "up", "output", "http", "Upstream servers it proxies to and balances across", { optional: true, multiplicity: "many" })], 1, 512, [
      prop("server-name", "Server name", "string", { default: "_", group: "Server" }),
      prop("channel", "Release channel", "enum", { options: ["stable", "mainline"], default: "stable", required: true, group: "Server", description: "Stable gets only critical fixes; mainline gets new features first." }),
      prop("workers", "Worker processes", "int", { default: 2, min: 1, max: 64, required: true, group: "Server", description: "One per vCPU is a good starting point." }),
      prop("gzip", "Gzip responses", "bool", { default: true, group: "Server" }),
      prop("tls", "Serve HTTPS", "bool", { default: false, group: "TLS" }),
      prop("http2", "Enable HTTP/2", "bool", { default: true, group: "TLS" }),
      prop("cert-secret", "Certificate secret reference", "secret", { default: "nginx-tls", group: "TLS", description: "Name of the secret that holds the certificate and private key." }),
      prop("worker-connections", "Connections per worker", "int", { default: 1024, min: 64, max: 65536, required: true, group: "Limits" }),
      prop("max-body-mib", "Max request body (MiB)", "int", { default: 1, min: 0, max: 10240, required: true, group: "Limits", description: "0 means no limit." }),
      prop("rate-limit", "Requests per second per client", "int", { default: 0, min: 0, max: 100000, group: "Limits", description: "0 turns rate limiting off." }),
      prop("lb-method", "Upstream balancing", "enum", { options: ["round-robin", "least-conn", "ip-hash"], default: "round-robin", required: true, group: "Proxy", description: "How requests are spread across the servers on the up port." }),
      prop("cache-mib", "Response cache (MiB)", "int", { default: 0, min: 0, max: 262144, group: "Proxy", description: "Caches upstream responses; 0 turns caching off." }),
    ]),
  cls("apache", "Apache httpd", "Web & Application", "Apache HTTP Server (httpd): modular web server and reverse proxy.", "apache",
    [p("http", "in", "input", "http", "HTTP requests"), p("db", "db", "output", "mysql", "MySQL-compatible connection", { optional: true, multiplicity: "one" }), p("upstream", "up", "output", "http", "Back-end servers it proxies to", { optional: true, multiplicity: "many" })], 1, 512, [
      prop("server-name", "Server name", "string", { default: "localhost", group: "Server" }),
      prop("mpm", "Processing model", "enum", { options: ["event", "worker", "prefork"], default: "event", required: true, group: "Server", description: "event suits most sites; prefork is for modules that are not thread-safe." }),
      prop("document-root", "Document root", "string", { default: "/var/www/html", group: "Server" }),
      prop("keepalive", "Keep connections alive", "bool", { default: true, group: "Server" }),
      prop("max-workers", "Max request workers", "int", { default: 150, min: 1, max: 10000, required: true, group: "Limits" }),
      prop("tls", "Serve HTTPS", "bool", { default: false, group: "TLS" }),
      prop("http2", "Enable HTTP/2", "bool", { default: true, group: "TLS" }),
      prop("cert-secret", "Certificate secret reference", "secret", { default: "httpd-tls", group: "TLS", description: "Name of the secret that holds the certificate and private key." }),
      prop("modules", "Extra modules", "string", { default: "", group: "Modules", description: "Space-separated module names to enable, such as rewrite or headers." }),
    ]),
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
  cls("linux-base", "Linux base", "Operating Systems", "Minimal Linux base image for custom modules.", "linux-base",
    [], 1, 512, [prop("hostname", "Hostname", "string", { default: "node", required: true, group: "System" }), prop("packages", "Extra packages", "string", { group: "System", description: "Space-separated package names to install." })]),
  ...EXTRA_CLASSES,
];

export const MODULE_CLASSES: readonly ModuleClass[] = BASE_CLASSES.map((c) => (RUNTIMES[c.id] ? { ...c, runtime: RUNTIMES[c.id] } : c));

const customClasses = new Map<string, ModuleClass>();

/** Replaces the user-defined (custom) classes known to lookups and validation. */
export function setCustomClasses(list: readonly ModuleClass[]): void {
  customClasses.clear();
  for (const c of list) customClasses.set(`${c.id}@${c.version}`, { ...c, custom: true });
}

export function allClasses(): ModuleClass[] {
  return [...MODULE_CLASSES, ...customClasses.values()];
}

const key = (name: string, version: string) => `${name}@${version}`;
const index = new Map(MODULE_CLASSES.map((c) => [key(c.id, c.version), c]));

export const lookupClass = (name: string, version: string): ModuleClass | undefined => index.get(key(name, version)) ?? customClasses.get(key(name, version));
