import type { ApplianceClass, Port, Property } from "@apelora/assembly-model";
import { cls, p, prop } from "./factory";

/**
 * Port placement convention (see docs/architecture/APPLIANCE-PORTS.md)
 *   Inputs  (left):  primary service input first, then secondary inputs; `net` (the NIC) is always last.
 *   Outputs (right): primary downstream first, then peer/replication links, then `mon` (metrics) last.
 * Names are short and lowercase so they fit beside the logo of a compact 128-px node.
 */
type Flags = "" | "o" | "1" | "o1";
const i = (id: string, type: Port["type"], desc: string, f: Flags = ""): Port => p(id, id, "input", type, desc, { optional: f.includes("o"), multiplicity: f.includes("1") ? "one" : "many" });
const o = (id: string, type: Port["type"], desc: string, f: Flags = "o"): Port => p(id, id, "output", type, desc, { optional: f.includes("o"), multiplicity: f.includes("1") ? "one" : "many" });
const mon = (): Port => o("mon", "metrics", "Metrics endpoint for Prometheus-style scraping");

const ver = (options: string[], def: string): Property => prop("version", "Version", "enum", { options, default: def, required: true, group: "Software" });
const sec = (id: string, label: string, def: string, group = "Access"): Property => prop(id, label, "secret", { default: def, required: true, group, description: "Name of the secret that holds it." });
const num = (id: string, label: string, def: number, min: number, max: number, group: string, desc?: string): Property => prop(id, label, "int", { default: def, min, max, required: true, group, ...(desc ? { description: desc } : {}) });
const txt = (id: string, label: string, def: string, group: string, desc?: string): Property => prop(id, label, "string", { default: def, group, ...(desc ? { description: desc } : {}) });
const flag = (id: string, label: string, def: boolean, group: string): Property => prop(id, label, "bool", { default: def, group });

/** Spec shorthand: id, name, category, summary, icon, inputs, outputs, vCPU, MiB, properties. */
const a = (id: string, name: string, cat: ApplianceClass["category"], summary: string, icon: string, ins: Port[], outs: Port[], cpu: number, mem: number, props: Property[] = []): ApplianceClass =>
  cls(id, name, cat, summary, icon, [...ins, ...outs], cpu, mem, props);

export const EXTRA_CLASSES: readonly ApplianceClass[] = [
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
  a("keycloak", "Keycloak", "Security & Identity", "Identity and access management (OIDC, SAML).", "keycloak",
    [i("http", "http", "Login and admin requests")], [o("db", "postgres", "Realm database", "o1"), mon()], 2, 2048,
    [txt("realm", "Default realm", "master", "Realm"), sec("admin-secret", "Admin password secret reference", "keycloak-admin"), flag("hostname-strict", "Strict hostname", true, "Server")]),
  a("vault", "Vault", "Security & Identity", "Secrets management and encryption service.", "vault",
    [i("api", "http", "Secret API")], [o("audit", "syslog", "Audit log stream"), mon()], 1, 1024,
    [flag("tls", "Require TLS", true, "Server"), num("default-ttl-h", "Default lease TTL (h)", 768, 1, 87600, "Leases"), sec("unseal-secret", "Unseal key secret reference", "vault-unseal")]),
  a("authelia", "Authelia", "Security & Identity", "Authentication and authorization server for reverse proxies.", "authelia",
    [i("http", "http", "Authentication requests")], [o("ldap", "ldap", "User directory", "o1"), o("cache", "redis", "Session storage", "o1")], 1, 256,
    [txt("domain", "Protected domain", "example.test", "Server"), prop("policy", "Default policy", "enum", { options: ["deny", "one_factor", "two_factor"], default: "two_factor", required: true, group: "Access" })]),

  // ---- Load balancing and proxies ----
  a("traefik", "Traefik", "Load Balancing", "Cloud-native reverse proxy and load balancer.", "traefikproxy",
    [i("in", "http", "Client traffic")], [o("out", "http", "Backend services"), mon()], 1, 256,
    [flag("tls", "Terminate TLS", true, "Entrypoints"), txt("entrypoints", "Entrypoints", "web, websecure", "Entrypoints"), flag("dashboard", "Enable dashboard", false, "Admin")]),
  a("envoy", "Envoy", "Load Balancing", "L4/L7 proxy and edge/service proxy.", "envoyproxy",
    [i("in", "http", "Client traffic")], [o("out", "http", "Upstream clusters"), mon()], 1, 256,
    [prop("lb-policy", "LB policy", "enum", { options: ["round_robin", "least_request", "ring_hash"], default: "round_robin", required: true, group: "Clusters" }), num("max-connections", "Max connections", 1024, 1, 1000000, "Limits")]),
  a("caddy", "Caddy", "Load Balancing", "Web server and reverse proxy with automatic HTTPS.", "caddy",
    [i("in", "http", "Client traffic")], [o("out", "http", "Upstream servers")], 1, 128,
    [txt("site", "Site address", "app.example.test", "Site"), flag("auto-https", "Automatic HTTPS", true, "Site")]),
  a("varnish", "Varnish", "Load Balancing", "HTTP accelerator and caching reverse proxy.", "generic-cache",
    [i("in", "http", "Client traffic")], [o("origin", "http", "Origin servers", "1"), mon()], 1, 1024,
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
    [i("http", "http", "HTTP requests")], [o("db", "postgres", "Database connection", "o1"), o("mq", "kafka", "Message stream", "o1")], 1, 1536,
    [ver(["17", "21", "25"], "21"), txt("jvm-opts", "JVM options", "-Xmx1g", "Runtime"), txt("jar", "Application JAR", "app.jar", "Runtime")]),
  a("wordpress", "WordPress", "Web & Application", "WordPress content management system.", "wordpress",
    [i("http", "http", "HTTP requests")], [o("db", "mysql", "Database", "1"), o("cache", "memcached", "Object cache", "o1")], 1, 1024,
    [txt("site-url", "Site URL", "https://blog.example.test", "Site"), sec("db-secret", "Database password secret reference", "wp-db-password"), flag("multisite", "Multisite", false, "Site")]),
  a("nextcloud", "Nextcloud", "Web & Application", "File sync and collaboration platform.", "nextcloud",
    [i("http", "http", "HTTP requests")], [o("db", "postgres", "Database", "1"), o("cache", "redis", "File locking and cache", "o1"), o("store", "s3", "Object storage", "o1")], 2, 2048,
    [txt("trusted-domain", "Trusted domain", "files.example.test", "Site"), sec("admin-secret", "Admin password secret reference", "nc-admin"), num("upload-mb", "Max upload (MiB)", 512, 1, 20480, "Limits")]),
  a("gitea", "Gitea", "Web & Application", "Self-hosted Git service.", "gitea",
    [i("http", "http", "Web and Git-over-HTTP requests")], [o("db", "postgres", "Database", "1")], 1, 1024,
    [txt("root-url", "Root URL", "https://git.example.test", "Server"), flag("registration", "Open registration", false, "Server"), sec("secret-key", "Secret key secret reference", "gitea-secret")]),
  a("gitlab", "GitLab", "Web & Application", "DevOps platform: Git, CI/CD and registry.", "gitlab",
    [i("http", "http", "Web and Git requests")], [o("db", "postgres", "Database", "1"), o("cache", "redis", "Cache and queues", "1"), o("store", "s3", "Object storage", "o1")], 4, 8192,
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
  a("influxdb", "InfluxDB", "Databases & Cache", "Time-series database.", "influxdb",
    [i("http", "http", "Write and query API")], [mon()], 2, 2048,
    [ver(["2.7", "3.0"], "2.7"), txt("org", "Organization", "apelora", "Tenant"), num("retention-days", "Retention (days)", 30, 0, 3650, "Data"), sec("token-secret", "Admin token secret reference", "influx-token")]),
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
    [i("in", "mqtt", "Publishers")], [o("out", "mqtt", "Subscribers", ""), mon()], 1, 256,
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
    [i("in", "syslog", "Log streams from appliances")], [o("out", "http", "Elasticsearch or OpenSearch", "1"), mon()], 2, 2048,
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
    [i("nfs", "nfs", "NFS clients")], [], 1, 512,
    [txt("export", "Export path", "/srv/share", "Export"), prop("access", "Access", "enum", { options: ["rw", "ro"], default: "rw", required: true, group: "Export" })]),

  // ---- Monitoring ----
  a("jaeger", "Jaeger", "Monitoring", "Distributed tracing.", "jaeger",
    [i("traces", "grpc", "Trace spans")], [o("store", "http", "Search backend", "o1")], 1, 1024,
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

  // ---- Utilities ----
  a("consul", "Consul", "Utilities", "Service discovery and configuration.", "consul",
    [i("api", "http", "HTTP and DNS API clients")], [mon()], 1, 1024,
    [txt("datacenter", "Datacenter", "dc1", "Cluster"), num("servers", "Expected servers", 3, 1, 9, "Cluster"), sec("gossip-secret", "Gossip key secret reference", "consul-gossip")]),
  a("etcd", "etcd", "Utilities", "Distributed key-value store.", "etcd",
    [i("kv", "grpc", "Key-value API clients")], [mon()], 1, 1024,
    [num("members", "Cluster members", 3, 1, 9, "Cluster"), num("quota-gb", "Backend quota (GiB)", 2, 1, 8, "Storage")]),
];
