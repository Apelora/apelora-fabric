import type { ModuleClass, Port, Property } from "@apelora/assembly-model";
import { MON, cls, p, prop } from "./factory";

/**
 * Port placement convention (see docs/architecture/MODULE-PORTS.md)
 *   Inputs  (left):  primary service input first, then secondary inputs; `net` (the NIC) is always last.
 *   Outputs (right): primary downstream first, then peer/replication links, then `mon` (metrics) last.
 * Names are short and lowercase so they fit beside the logo of a compact 128-px node.
 */
type Flags = "" | "o" | "1" | "o1";
const i = (id: string, type: Port["type"], desc: string, f: Flags = ""): Port => p(id, id, "input", type, desc, { optional: f.includes("o"), multiplicity: f.includes("1") ? "one" : "many" });
const o = (id: string, type: Port["type"], desc: string, f: Flags = "o"): Port => p(id, id, "output", type, desc, { optional: f.includes("o"), multiplicity: f.includes("1") ? "one" : "many" });
const mon = (): Port => MON();

const ver = (options: string[], def: string): Property => prop("version", "Version", "enum", { options, default: def, required: true, group: "Software" });
const sec = (id: string, label: string, def: string, group = "Access"): Property => prop(id, label, "secret", { default: def, required: true, group, description: "Name of the secret that holds it." });
const num = (id: string, label: string, def: number, min: number, max: number, group: string, desc?: string): Property => prop(id, label, "int", { default: def, min, max, required: true, group, ...(desc ? { description: desc } : {}) });
const txt = (id: string, label: string, def: string, group: string, desc?: string): Property => prop(id, label, "string", { default: def, group, ...(desc ? { description: desc } : {}) });
const flag = (id: string, label: string, def: boolean, group: string): Property => prop(id, label, "bool", { default: def, group });

/** Spec shorthand: id, name, category, summary, icon, inputs, outputs, vCPU, MiB, properties. */
const a = (id: string, name: string, cat: ModuleClass["category"], summary: string, icon: string, ins: Port[], outs: Port[], cpu: number, mem: number, props: Property[] = []): ModuleClass =>
  cls(id, name, cat, summary, icon, [...ins, ...outs], cpu, mem, props);

export const EXTRA_CLASSES: readonly ModuleClass[] = [
  // ---- Network & Security ----
  a("pfsense", "pfSense", "Network & Security", "Open-source firewall and router.", "pfsense",
    [i("wan", "network", "Uplink", "o1")], [o("lan", "network", "Protected network", "")], 2, 1024,
    [prop("default-policy", "Default policy", "enum", { options: ["deny", "allow"], default: "deny", required: true, group: "Firewall" }), sec("admin-secret", "Admin password reference", "firewall-admin")]),
  a("wireguard", "WireGuard", "Network & Security", "VPN gateway using WireGuard tunnels.", "wireguard",
    [i("wan", "network", "Public side, where tunnels arrive", "o1")], [o("lan", "network", "Private network behind the tunnel", "")], 1, 256,
    [num("listen-port", "Listen port", 51820, 1, 65535, "Tunnel"), txt("allowed-ips", "Allowed IPs", "10.8.0.0/24", "Tunnel"), sec("key-secret", "Private key secret reference", "wg-key")]),
  a("openvpn", "OpenVPN", "Network & Security", "SSL VPN server.", "openvpn",
    [i("wan", "network", "Public side, where clients connect", "o1")], [o("lan", "network", "Private network behind the VPN", "")], 1, 512,
    [num("port", "Port", 1194, 1, 65535, "Server"), prop("proto", "Protocol", "enum", { options: ["udp", "tcp"], default: "udp", group: "Server" }), sec("ca-secret", "CA key secret reference", "vpn-ca")]),

  // ---- Security & Identity ----
  a("keycloak", "Keycloak", "Security & Identity", "Identity and access management: single sign-on (OIDC, SAML), user federation, MFA and fine-grained authorization.", "keycloak",
    [i("http", "http", "Login and admin requests")], [o("db", "postgres", "Realm database", "o1"), o("ldap", "ldap", "User federation: an existing LDAP or directory server", "o1"), mon()], 2, 2048,
    [txt("realm", "Default realm", "master", "Realm"), num("sso-idle-min", "SSO session idle (min)", 30, 1, 10080, "Realm"), flag("brute-force", "Brute-force detection", true, "Realm"),
      sec("admin-secret", "Admin password secret reference", "keycloak-admin"),
      txt("hostname", "Public host name", "sso.example.test", "Server", "Name users and applications reach the login pages on."), flag("hostname-strict", "Strict hostname", true, "Server"),
      prop("start-mode", "Start mode", "enum", { options: ["production", "development"], default: "production", required: true, group: "Server", description: "Development mode relaxes TLS and caching; never use it in production." }),
      prop("proxy-headers", "Proxy headers", "enum", { options: ["none", "forwarded", "xforwarded"], default: "none", required: true, group: "Server", description: "Trust client address headers set by a reverse proxy in front." }),
      flag("health", "Health endpoints", true, "Observability"), flag("metrics", "Metrics endpoint", true, "Observability")]),
  a("vault", "Vault", "Security & Identity", "Secrets management and encryption service.", "vault",
    [i("api", "http", "Secret API")], [o("audit", "syslog", "Audit log stream"), mon()], 1, 1024,
    [flag("tls", "Require TLS", true, "Server"), num("default-ttl-h", "Default lease TTL (h)", 768, 1, 87600, "Leases"), sec("unseal-secret", "Unseal key secret reference", "vault-unseal")]),
  a("authelia", "Authelia", "Security & Identity", "Authentication and authorization server for reverse proxies.", "authelia",
    [i("http", "http", "Authentication requests")], [o("ldap", "ldap", "User directory", "o1"), o("cache", "redis", "Session storage", "o1"), mon()], 1, 256,
    [txt("domain", "Protected domain", "example.test", "Server"), prop("policy", "Default policy", "enum", { options: ["deny", "one_factor", "two_factor"], default: "two_factor", required: true, group: "Access" })]),

  a("authentik", "Authentik", "Security & Identity", "Identity provider and SSO (OIDC, SAML, LDAP, proxy outposts).", "authentik",
    [i("http", "http", "Login, admin and SSO requests")], [o("db", "postgres", "Database", "o1"), o("cache", "redis", "Cache and task queue", "o1"), mon()], 2, 2048,
    [txt("host", "Public host name", "auth.example.test", "Server"), sec("secret-key", "Secret key secret reference", "authentik-secret"), sec("bootstrap-secret", "Bootstrap password secret reference", "authentik-bootstrap"), flag("error-reporting", "Send error reports", false, "Server")]),
  a("pocketid", "Pocket ID", "Security & Identity", "Simple OIDC provider with passkey-only sign-in.", "generic-passkey",
    [i("http", "http", "OIDC and admin requests")], [o("db", "postgres", "Database (SQLite by default)", "o1"), mon()], 1, 256,
    [txt("app-url", "Public URL", "https://id.example.test", "Server"), flag("allow-signup", "Allow user sign-up", false, "Access"), sec("encryption-secret", "Encryption key secret reference", "pocketid-key")]),
  a("openldap", "OpenLDAP", "Security & Identity", "LDAP directory server.", "generic-directory",
    [i("ldap", "ldap", "Directory clients")], [o("repl", "ldap", "Replication to other directory servers")], 1, 512,
    [txt("base-dn", "Base DN", "dc=example,dc=test", "Directory"), sec("admin-secret", "Admin password secret reference", "ldap-admin"), flag("tls", "Require TLS", true, "Directory")]),
  a("zitadel", "Zitadel", "Security & Identity", "Identity infrastructure: SSO, MFA and multi-tenant IAM.", "generic-shield",
    [i("http", "http", "Login, API and console requests")], [o("db", "postgres", "Database", "1"), mon()], 2, 1024,
    [txt("external-domain", "External domain", "id.example.test", "Server"), sec("master-secret", "Master key secret reference", "zitadel-master")]),

  // ---- Load balancing and proxies ----
  a("traefik", "Traefik", "Load Balancing", "Cloud-native reverse proxy and load balancer.", "traefikproxy",
    [i("in", "http", "Client traffic")], [o("out", "http", "Backend services"), mon()], 1, 256,
    [flag("tls", "Terminate TLS", true, "Entrypoints"), txt("entrypoints", "Entrypoints", "web, websecure", "Entrypoints"), flag("dashboard", "Enable dashboard", false, "Admin")]),
  a("envoy", "Envoy", "Load Balancing", "L4/L7 proxy and edge/service proxy.", "envoyproxy",
    [i("in", "http", "Client traffic")], [o("out", "http", "Upstream clusters"), mon()], 1, 256,
    [prop("lb-policy", "LB policy", "enum", { options: ["round_robin", "least_request", "ring_hash"], default: "round_robin", required: true, group: "Clusters" }), num("max-connections", "Max connections", 1024, 1, 1000000, "Limits")]),
  a("caddy", "Caddy", "Load Balancing", "Web server and reverse proxy with automatic HTTPS.", "caddy",
    [i("in", "http", "Client traffic")], [o("out", "http", "Upstream servers"), mon()], 1, 128,
    [txt("site", "Site address", "app.example.test", "Site"), flag("auto-https", "Automatic HTTPS", true, "Site")]),
  a("varnish", "Varnish", "Load Balancing", "HTTP accelerator and caching reverse proxy.", "generic-cache",
    [i("in", "http", "Client traffic")], [o("origin", "http", "Origin servers", "1")], 1, 1024,
    [num("cache-mb", "Cache size (MiB)", 512, 16, 262144, "Cache"), num("ttl-s", "Default TTL (s)", 120, 0, 86400, "Cache")]),

  // ---- Web & Application ----
  a("tomcat", "Tomcat", "Web & Application", "Apache Tomcat Java servlet container.", "apachetomcat",
    [i("http", "http", "HTTP requests")], [o("db", "postgres", "Database connection", "o1"), mon()], 1, 1536,
    [ver(["9", "10.1", "11"], "10.1"), txt("java-opts", "JVM options", "-Xmx1g", "Runtime"), num("max-threads", "Max threads", 200, 10, 5000, "Limits")]),
  a("php", "PHP-FPM", "Web & Application", "PHP application runtime (FastCGI).", "php",
    [i("http", "http", "Requests from the web server")], [o("db", "mysql", "Database connection", "o1"), o("cache", "memcached", "Cache connection", "o1")], 1, 512,
    [ver(["8.2", "8.3", "8.4"], "8.3"), num("memory-limit", "Memory limit (MiB)", 256, 32, 4096, "Runtime"), num("max-children", "Max children", 20, 1, 500, "Limits")]),
  a("python", "Python app", "Web & Application", "Python application server (WSGI/ASGI).", "python",
    [i("http", "http", "HTTP requests")], [o("db", "postgres", "Database connection", "o1"), o("cache", "redis", "Cache connection", "o1")], 1, 512,
    [ver(["3.11", "3.12", "3.13"], "3.12"), txt("module", "App module", "app:app", "Runtime"), num("workers", "Workers", 2, 1, 64, "Runtime")]),
  a("openjdk", "Java app", "Web & Application", "Java application on OpenJDK.", "openjdk",
    [i("http", "http", "HTTP requests")], [o("db", "postgres", "Database connection", "o1"), o("mq", "kafka", "Message stream", "o1"), mon()], 1, 1536,
    [ver(["17", "21", "25"], "21"), txt("jvm-opts", "JVM options", "-Xmx1g", "Runtime"), txt("jar", "Application JAR", "app.jar", "Runtime")]),
  a("wordpress", "WordPress", "Web & Application", "WordPress content management system.", "wordpress",
    [i("http", "http", "HTTP requests")], [o("db", "mysql", "Database", "1"), o("cache", "memcached", "Object cache", "o1")], 1, 1024,
    [txt("site-url", "Site URL", "https://blog.example.test", "Site"), sec("db-secret", "Database password secret reference", "wp-db-password"), flag("multisite", "Multisite", false, "Site")]),
  a("nextcloud", "Nextcloud", "Web & Application", "File sync and collaboration platform.", "nextcloud",
    [i("http", "http", "HTTP requests")], [o("db", "postgres", "Database", "1"), o("cache", "redis", "File locking and cache", "o1"), o("store", "s3", "Object storage", "o1"), mon()], 2, 2048,
    [txt("trusted-domain", "Trusted domain", "files.example.test", "Site"), sec("admin-secret", "Admin password secret reference", "nc-admin"), num("upload-mb", "Max upload (MiB)", 512, 1, 20480, "Limits")]),
  a("gitea", "Gitea", "Web & Application", "Self-hosted Git service.", "gitea",
    [i("http", "http", "Web and Git-over-HTTP requests")], [o("db", "postgres", "Database", "1"), mon()], 1, 1024,
    [txt("root-url", "Root URL", "https://git.example.test", "Server"), flag("registration", "Open registration", false, "Server"), sec("secret-key", "Secret key secret reference", "gitea-secret")]),
  a("gitlab", "GitLab", "Web & Application", "DevOps platform: Git, CI/CD and registry.", "gitlab",
    [i("http", "http", "Web and Git requests")], [o("db", "postgres", "Database", "1"), o("cache", "redis", "Cache and queues", "1"), o("store", "s3", "Object storage", "o1"), mon()], 4, 8192,
    [txt("external-url", "External URL", "https://gitlab.example.test", "Server"), sec("root-secret", "Root password secret reference", "gitlab-root"), num("puma-workers", "Puma workers", 2, 1, 32, "Limits")]),
  a("jenkins", "Jenkins", "Web & Application", "Automation server for CI/CD.", "jenkins",
    [i("http", "http", "Web UI and webhooks")], [mon()], 2, 2048,
    [num("executors", "Executors", 2, 0, 64, "Build"), txt("jenkins-url", "Jenkins URL", "https://ci.example.test", "Server"), sec("admin-secret", "Admin password secret reference", "jenkins-admin")]),

  // ---- Databases & cache ----
  a("mysql", "MySQL", "Databases & Cache", "Relational database.", "mysql",
    [i("sql", "mysql", "Client connections")], [o("repl", "mysql", "Replication to replicas"), mon()], 2, 2048,
    [ver(["8.0", "8.4", "9.0"], "8.4"), txt("database", "Database name", "app", "Database"), num("max-connections", "Max connections", 151, 10, 10000, "Limits"), sec("root-secret", "Root password secret reference", "mysql-root")]),
  a("mongodb", "MongoDB", "Databases & Cache", "Document database.", "mongodb",
    [i("mongo", "mongodb", "Client connections")], [o("repl", "mongodb", "Replica-set members"), mon()], 2, 4096,
    [ver(["6.0", "7.0", "8.0"], "7.0"), txt("replica-set", "Replica set name", "rs0", "Cluster"), num("cache-gb", "WiredTiger cache (GiB)", 1, 1, 256, "Memory"), sec("admin-secret", "Admin password secret reference", "mongo-admin")]),
  a("couchdb", "CouchDB", "Databases & Cache", "Apache CouchDB document database with HTTP API.", "apachecouchdb",
    [i("http", "http", "HTTP API clients")], [o("repl", "http", "Replication to other CouchDB nodes"), mon()], 2, 2048,
    [ver(["3.3", "3.4"], "3.4"), num("q", "Shards per database (q)", 2, 1, 64, "Cluster"), num("n", "Replicas (n)", 3, 1, 5, "Cluster"), sec("admin-secret", "Admin password secret reference", "couchdb-admin")]),
  a("cassandra", "Cassandra", "Databases & Cache", "Apache Cassandra wide-column database.", "apachecassandra",
    [i("cql", "cassandra", "CQL client connections")], [o("peer", "cassandra", "Gossip with other nodes"), mon()], 4, 8192,
    [ver(["4.1", "5.0"], "5.0"), txt("cluster-name", "Cluster name", "apelora", "Cluster"), num("replication-factor", "Replication factor", 3, 1, 9, "Cluster")]),
  a("scylladb", "ScyllaDB", "Databases & Cache", "Cassandra-compatible high-performance database.", "scylladb",
    [i("cql", "cassandra", "CQL client connections")], [o("peer", "cassandra", "Gossip with other nodes"), mon()], 4, 8192,
    [txt("cluster-name", "Cluster name", "apelora", "Cluster"), num("smp", "CPU shards", 2, 1, 128, "Performance")]),
  a("cockroachdb", "CockroachDB", "Databases & Cache", "Distributed SQL database (PostgreSQL wire protocol).", "cockroachlabs",
    [i("sql", "postgres", "SQL client connections")], [o("peer", "tcp", "Cluster traffic with other nodes"), mon()], 4, 8192,
    [num("replicas", "Replicas", 3, 1, 7, "Cluster"), txt("locality", "Locality", "region=local", "Cluster"), flag("insecure", "Insecure mode (lab only)", false, "Security")]),
  a("clickhouse", "ClickHouse", "Databases & Cache", "Column-oriented analytics database.", "clickhouse",
    [i("http", "http", "HTTP and SQL clients")], [mon()], 4, 8192,
    [ver(["24.8", "25.3"], "25.3"), num("max-memory-gb", "Max memory (GiB)", 4, 1, 1024, "Limits"), sec("default-secret", "Default user password secret reference", "ch-default")]),
  a("influxdb", "InfluxDB", "Databases & Cache", "Time-series database for metrics, events and sensor data, with SQL queries and optional object-storage persistence.", "influxdb",
    [i("http", "http", "Write and query API")], [o("store", "s3", "Object storage for data files (local disk when unplugged)", "o1"), mon()], 2, 2048,
    [ver(["2.7", "3.0"], "2.7"), txt("node-id", "Node ID", "influx-01", "Software", "Identifies this server's data in shared storage."),
      txt("org", "Organization", "apelora", "Tenant"), sec("token-secret", "Admin token secret reference", "influx-token"),
      num("retention-days", "Retention (days)", 30, 0, 3650, "Data"),
      prop("storage", "Data files", "enum", { options: ["local", "object"], default: "local", required: true, group: "Data", description: "Keep data files on local disk or in object storage on the store port." }),
      flag("compat-api", "Accept older write and query APIs", true, "Compatibility"),
      flag("plugins", "Processing engine (Python plugins)", false, "Plugins")]),
  a("neo4j", "Neo4j", "Databases & Cache", "Graph database.", "neo4j",
    [i("bolt", "tcp", "Bolt and HTTP clients")], [mon()], 2, 4096,
    [ver(["5.26", "2025.01"], "5.26"), num("heap-mb", "Heap size (MiB)", 1024, 256, 65536, "Memory"), sec("auth-secret", "Password secret reference", "neo4j-auth")]),
  a("arangodb", "ArangoDB", "Databases & Cache", "Multi-model (document, graph, key/value) database.", "arangodb",
    [i("http", "http", "HTTP API clients")], [mon()], 2, 2048,
    [txt("database", "Database name", "app", "Database"), sec("root-secret", "Root password secret reference", "arango-root")]),
  a("memcached", "Memcached", "Databases & Cache", "In-memory key-value cache.", "generic-cache",
    [i("mc", "memcached", "Cache clients")], [mon()], 1, 512,
    [num("memory-mb", "Memory (MiB)", 256, 16, 65536, "Memory"), num("max-conn", "Max connections", 1024, 16, 100000, "Limits")]),

  // ---- Messaging & streaming ----
  a("kafka", "Kafka", "Messaging & Streaming", "Apache Kafka event streaming platform (KRaft).", "apachekafka",
    [i("in", "kafka", "Producers")], [o("out", "kafka", "Consumers", ""), mon()], 4, 8192,
    [ver(["3.8", "3.9", "4.0"], "3.9"), num("partitions", "Default partitions", 3, 1, 1000, "Topics"), num("replication", "Replication factor", 3, 1, 9, "Topics"), num("retention-h", "Retention (h)", 168, 1, 87600, "Topics")]),
  a("pulsar", "Pulsar", "Messaging & Streaming", "Apache Pulsar messaging and streaming platform.", "apachepulsar",
    [i("in", "pulsar", "Producers")], [o("out", "pulsar", "Consumers", ""), mon()], 4, 6144,
    [txt("cluster", "Cluster name", "apelora", "Cluster"), num("retention-h", "Retention (h)", 72, 1, 87600, "Topics")]),
  a("nats", "NATS", "Messaging & Streaming", "NATS messaging system.", "generic-mq",
    [i("in", "nats", "Publishers")], [o("out", "nats", "Subscribers", ""), mon()], 1, 512,
    [flag("jetstream", "JetStream persistence", true, "Server"), num("max-payload-kb", "Max payload (KiB)", 1024, 1, 65536, "Limits")]),
  a("mosquitto", "Mosquitto", "Messaging & Streaming", "Eclipse Mosquitto MQTT broker.", "eclipsemosquitto",
    [i("in", "mqtt", "Publishers")], [o("out", "mqtt", "Subscribers", "")], 1, 256,
    [flag("allow-anonymous", "Allow anonymous clients", false, "Access"), num("max-connections", "Max connections", 1000, 1, 100000, "Limits")]),
  a("activemq", "ActiveMQ", "Messaging & Streaming", "Apache ActiveMQ message broker.", "generic-mq",
    [i("in", "amqp", "Producers")], [o("out", "amqp", "Consumers", ""), mon()], 2, 2048,
    [prop("flavour", "Broker", "enum", { options: ["classic", "artemis"], default: "artemis", required: true, group: "Software" }), sec("admin-secret", "Admin password secret reference", "mq-admin")]),

  // ---- Search & analytics ----
  a("elasticsearch", "Elasticsearch", "Search & Analytics", "Distributed search and analytics engine.", "elasticsearch",
    [i("http", "http", "Index and search API")], [mon()], 4, 8192,
    [ver(["8.15", "8.17", "9.0"], "8.17"), num("heap-gb", "Heap size (GiB)", 4, 1, 31, "Memory"), num("shards", "Primary shards", 1, 1, 1000, "Index"), sec("elastic-secret", "elastic user password secret reference", "es-elastic")]),
  a("opensearch", "OpenSearch", "Search & Analytics", "Community search and analytics suite.", "opensearch",
    [i("http", "http", "Index and search API")], [mon()], 4, 8192,
    [ver(["2.15", "2.19", "3.0"], "2.19"), num("heap-gb", "Heap size (GiB)", 4, 1, 31, "Memory"), sec("admin-secret", "Admin password secret reference", "os-admin")]),
  a("solr", "Solr", "Search & Analytics", "Apache Solr search platform.", "apachesolr",
    [i("http", "http", "Search and admin API")], [mon()], 2, 4096,
    [ver(["9.6", "9.8"], "9.8"), txt("core", "Default core", "main", "Index"), num("heap-mb", "Heap (MiB)", 1024, 256, 32768, "Memory")]),
  a("kibana", "Kibana", "Search & Analytics", "Dashboards for Elasticsearch.", "kibana",
    [i("ui", "http", "Browser traffic")], [o("es", "http", "Elasticsearch cluster", "1")], 1, 1024,
    [txt("base-path", "Base path", "/", "Server"), sec("service-secret", "Service account secret reference", "kibana-service")]),
  a("logstash", "Logstash", "Search & Analytics", "Log ingestion and transformation pipeline.", "logstash",
    [i("in", "syslog", "Log streams from modules")], [o("out", "http", "Elasticsearch or OpenSearch", "1")], 2, 2048,
    [num("workers", "Pipeline workers", 2, 1, 64, "Pipeline"), num("batch-size", "Batch size", 125, 1, 10000, "Pipeline")]),
  a("spark", "Spark", "Search & Analytics", "Apache Spark analytics engine (driver and workers).", "apachespark",
    [i("submit", "http", "Job submission and UI")], [o("store", "s3", "Data lake", "o1"), mon()], 4, 8192,
    [ver(["3.5", "4.0"], "3.5"), num("executors", "Executors", 2, 1, 256, "Cluster"), num("executor-mem-gb", "Executor memory (GiB)", 4, 1, 512, "Cluster")]),
  a("airflow", "Airflow", "Search & Analytics", "Apache Airflow workflow scheduler.", "apacheairflow",
    [i("ui", "http", "Web UI")], [o("db", "postgres", "Metadata database", "1"), o("mq", "redis", "Task queue", "o1")], 2, 3072,
    [prop("executor", "Executor", "enum", { options: ["local", "celery", "kubernetes"], default: "celery", required: true, group: "Scheduler" }), sec("fernet-secret", "Fernet key secret reference", "airflow-fernet")]),

  // ---- Storage ----
  a("ceph", "Ceph", "Storage", "Distributed object and block storage.", "ceph",
    [i("s3", "s3", "S3 gateway clients")], [mon()], 4, 8192,
    [num("replicas", "Replica count", 3, 1, 9, "Pools"), num("pg-num", "Placement groups", 128, 8, 8192, "Pools")]),
  a("nfs", "NFS server", "Storage", "Network file system exports.", "generic-storage",
    [i("nfs", "nfs", "NFS clients")], [mon()], 1, 512,
    [txt("export", "Export path", "/srv/share", "Export"), prop("access", "Access", "enum", { options: ["rw", "ro"], default: "rw", required: true, group: "Export" })]),

  // ---- Monitoring ----
  a("jaeger", "Jaeger", "Monitoring", "Distributed tracing.", "jaeger",
    [i("traces", "grpc", "Trace spans")], [o("store", "http", "Search backend", "o1"), mon()], 1, 1024,
    [num("sampling", "Sampling rate (%)", 10, 0, 100, "Collection"), num("ttl-h", "Trace retention (h)", 72, 1, 8760, "Storage")]),
  a("otel", "OTel Collector", "Monitoring", "OpenTelemetry Collector for traces, metrics and logs.", "opentelemetry",
    [i("otlp", "grpc", "OTLP telemetry from applications")], [o("traces", "grpc", "Trace backend", "o"), mon()], 1, 512,
    [flag("batch", "Batch processor", true, "Pipeline"), num("memory-limit-mb", "Memory limit (MiB)", 400, 64, 16384, "Limits")]),

  // ---- Operating systems ----
  a("debian", "Debian", "Operating Systems", "Debian Linux base image.", "debian", [], [], 1, 512, [ver(["12", "13"], "12"), txt("hostname", "Hostname", "node", "System"), txt("packages", "Extra packages", "", "System")]),
  a("ubuntu", "Ubuntu", "Operating Systems", "Ubuntu Server base image.", "ubuntu", [], [], 1, 512, [ver(["22.04", "24.04"], "24.04"), txt("hostname", "Hostname", "node", "System"), txt("packages", "Extra packages", "", "System")]),
  a("alpine", "Alpine Linux", "Operating Systems", "Minimal Alpine Linux base image.", "alpinelinux", [], [], 1, 256, [ver(["3.19", "3.20", "3.21"], "3.21"), txt("hostname", "Hostname", "node", "System"), txt("packages", "Extra packages", "", "System")]),
  a("rocky", "Rocky Linux", "Operating Systems", "Rocky Linux base image (RHEL-compatible).", "rockylinux", [], [], 1, 512, [ver(["8", "9"], "9"), txt("hostname", "Hostname", "node", "System")]),
  a("almalinux", "AlmaLinux", "Operating Systems", "AlmaLinux base image (RHEL-compatible).", "almalinux", [], [], 1, 512, [ver(["8", "9"], "9"), txt("hostname", "Hostname", "node", "System")]),
  a("fedora", "Fedora", "Operating Systems", "Fedora Linux base image.", "fedora", [], [], 1, 512, [ver(["40", "41", "42"], "42"), txt("hostname", "Hostname", "node", "System")]),

  // ---- Microsoft Windows (no Microsoft artwork is bundled; a valid Microsoft licence is required to run these) ----
  a("windows-server", "Windows Server", "Microsoft Windows", "Windows Server base image. A valid Microsoft licence is required; no image is bundled.", "generic-window", [], [], 2, 4096,
    [prop("version", "Version", "enum", { options: ["2019", "2022", "2025"], default: "2022", required: true, group: "Software" }), prop("edition", "Edition", "enum", { options: ["Standard", "Datacenter"], default: "Standard", required: true, group: "Software" }), txt("hostname", "Computer name", "WIN-SRV", "System"), sec("license-secret", "Licence key secret reference", "windows-license"), sec("admin-secret", "Administrator password secret reference", "windows-admin")]),
  a("windows-11", "Windows 11", "Microsoft Windows", "Windows 11 desktop client. A valid Microsoft licence is required; no image is bundled.", "generic-desktop",
    [i("rdp", "rdp", "Remote Desktop sessions", "o")], [], 2, 4096,
    [prop("version", "Version", "enum", { options: ["23H2", "24H2"], default: "24H2", required: true, group: "Software" }), txt("hostname", "Computer name", "WIN11-PC", "System"), sec("license-secret", "Licence key secret reference", "windows-license")]),
  a("windows-10", "Windows 10", "Microsoft Windows", "Windows 10 desktop client. A valid Microsoft licence is required; no image is bundled.", "generic-desktop",
    [i("rdp", "rdp", "Remote Desktop sessions", "o")], [], 2, 4096,
    [prop("version", "Version", "enum", { options: ["22H2"], default: "22H2", required: true, group: "Software" }), txt("hostname", "Computer name", "WIN10-PC", "System"), sec("license-secret", "Licence key secret reference", "windows-license")]),
  a("active-directory", "Active Directory", "Microsoft Windows", "Windows domain controller: directory, authentication and DNS.", "generic-directory",
    [i("ldap", "ldap", "Directory and authentication clients")], [o("repl", "ldap", "Replication with other domain controllers"), mon()], 2, 4096,
    [txt("domain", "Domain name", "corp.example.test", "Domain"), prop("functional-level", "Functional level", "enum", { options: ["2016", "2019", "2025"], default: "2016", required: true, group: "Domain" }), sec("dsrm-secret", "Recovery password secret reference", "ad-dsrm"), sec("license-secret", "Licence key secret reference", "windows-license")]),
  a("iis", "IIS", "Microsoft Windows", "Internet Information Services web server on Windows Server.", "generic-server",
    [i("http", "http", "HTTP requests")], [o("db", "mssql", "SQL Server connection", "o1"), o("auth", "ldap", "Active Directory", "o1"), mon()], 2, 4096,
    [txt("site", "Site name", "Default Web Site", "Site"), num("app-pool-limit", "App pool memory limit (MiB)", 2048, 128, 65536, "Limits"), flag("windows-auth", "Windows authentication", true, "Site")]),
  a("sql-server", "SQL Server", "Microsoft Windows", "Microsoft SQL Server database engine.", "generic-database",
    [i("sql", "mssql", "Client connections")], [o("repl", "mssql", "Availability group replicas"), mon()], 4, 8192,
    [prop("version", "Version", "enum", { options: ["2019", "2022"], default: "2022", required: true, group: "Software" }), prop("edition", "Edition", "enum", { options: ["Express", "Standard", "Enterprise"], default: "Standard", required: true, group: "Software" }), num("max-memory-mb", "Max server memory (MiB)", 4096, 512, 1048576, "Memory"), sec("sa-secret", "sa password secret reference", "mssql-sa")]),
  a("dotnet", ".NET app", "Microsoft Windows", ".NET application (ASP.NET Core) on Windows or Linux.", "dotnet",
    [i("http", "http", "HTTP requests")], [o("db", "mssql", "SQL Server connection", "o1"), o("cache", "redis", "Cache connection", "o1")], 1, 1024,
    [prop("runtime", "Runtime", "enum", { options: ["8.0", "9.0"], default: "8.0", required: true, group: "Runtime" }), txt("entry-dll", "Entry assembly", "App.dll", "Runtime"), prop("environment", "Environment", "enum", { options: ["Production", "Staging", "Development"], default: "Production", group: "Runtime" })]),
  a("smb-files", "File server", "Microsoft Windows", "Windows file server sharing folders over SMB.", "generic-storage",
    [i("smb", "smb", "SMB clients")], [mon()], 2, 2048,
    [txt("share", "Share name", "Data", "Share"), flag("encrypt", "Require SMB encryption", true, "Share"), num("quota-gb", "Quota (GiB)", 500, 1, 1048576, "Share")]),
  a("rd-gateway", "RD Gateway", "Microsoft Windows", "Remote Desktop Gateway: RDP over HTTPS.", "generic-window",
    [i("https", "http", "Client connections over HTTPS")], [o("rdp", "rdp", "Remote desktops behind the gateway"), o("auth", "ldap", "Active Directory", "o1"), mon()], 2, 2048,
    [txt("host", "Public host name", "rdg.example.test", "Gateway"), flag("mfa", "Require multi-factor authentication", true, "Gateway")]),
  a("exchange", "Exchange", "Microsoft Windows", "Microsoft Exchange Server mail platform.", "generic-window",
    [i("smtp", "smtp", "Incoming mail"), i("owa", "http", "Web and ActiveSync clients")], [o("auth", "ldap", "Active Directory", "1"), mon()], 4, 16384,
    [txt("org", "Organisation name", "Corp", "Organisation"), prop("version", "Version", "enum", { options: ["2019", "SE"], default: "SE", required: true, group: "Software" }), sec("license-secret", "Licence key secret reference", "exchange-license")]),

  // ---- Utilities ----
  a("consul", "Consul", "Utilities", "Service discovery and configuration.", "consul",
    [i("api", "http", "HTTP and DNS API clients")], [mon()], 1, 1024,
    [txt("datacenter", "Datacenter", "dc1", "Cluster"), num("servers", "Expected servers", 3, 1, 9, "Cluster"), sec("gossip-secret", "Gossip key secret reference", "consul-gossip")]),
  a("etcd", "etcd", "Utilities", "Distributed key-value store.", "etcd",
    [i("kv", "grpc", "Key-value API clients")], [mon()], 1, 1024,
    [num("members", "Cluster members", 3, 1, 9, "Cluster"), num("quota-gb", "Backend quota (GiB)", 2, 1, 8, "Storage")]),
];
