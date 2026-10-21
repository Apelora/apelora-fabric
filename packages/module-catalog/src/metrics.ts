import type { Metrics } from "@apelora/assembly-model";

/**
 * How each catalog class's software exposes its own metrics, researched against the projects'
 * documentation in 2026-10 and written in our own words. `confidence` says how well each entry
 * was checked; "unverified" entries must be tried before anything relies on them. A class whose
 * metrics a collector can read as text on a port offers a `mon` output, and a drawn `mon`
 * connection opens exactly `port` (docs/architecture/MODULE-PORTS.md, "Metrics").
 */
type Source = Metrics["source"];
type Format = Metrics["format"];
type Rest = Partial<Omit<Metrics, "source" | "format" | "confidence" | "signals">>;

const M = (source: Source, format: Format, confidence: Metrics["confidence"], signals: string[], rest: Rest = {}): Metrics => ({
  source, format, onByDefault: false, auth: "none", separateListener: false, signals, confidence, ...rest,
});

// ---- Shared wording ----
const NO_AUTH = "It has no authentication of its own: keep it on the application network, reachable only through the drawn mon connection.";
const JVM_AGENT = (extra: string) => `Attach the JVM metrics agent through the JVM options; it runs inside the JVM and serves metrics text on the port given in its argument (the class uses 9404, the port in the project's examples). ${extra}`;
const LINUX_AGENT = "Optional: install the distribution's node metrics agent package (it serves on 9100), then add a mon output to the module so a collector may read it.";
const RHEL_AGENT = "Optional: install the distribution's performance tools with their web proxy, which serves metrics text on 44322 (or the node metrics agent on 9100), then add a mon output to the module.";
const INCUS_CT = "Incus already reports this instance's CPU, memory, disk, network and processes. Inside a system container much of /proc is the host's view, filtered, so Incus's numbers are the reliable ones.";
const INCUS_VM = "Runs as a virtual machine. Incus already reports its CPU, memory, disk and network: through its agent inside the guest, or without the agent QEMU-level numbers only (memory estimated from the host, no filesystems or processes).";
const OS_SIGNALS = ["Filesystem space left", "Memory available and out-of-memory kills", "CPU time", "Disk and network errors", "Process count"];
const WIN_EXPORTER = (collector: string) => `Install a Windows metrics exporter service in the guest and turn on its ${collector} collector (off by default); it reads the performance counters and serves metrics text on 9182.`;
const WIN_VM = "Windows runs as a virtual machine. Whether Incus's agent runs in a Windows guest was not checked: expect QEMU-level numbers from Incus (CPU, estimated memory, disk, network). Counter names are translated on non-English installations.";
const WIN_DESKTOP = "Optional: install a Windows metrics exporter service in the guest (the common one serves on 9182), then add a mon output. Performance counters are always readable inside Windows.";
const FREEBSD = "FreeBSD-based: runs only as a virtual machine, and without an agent in the guest Incus sees QEMU-level numbers only.";

export const METRICS: Readonly<Record<string, Metrics>> = {
  // ---- Network & Security ----
  gateway: M("platform", "none", "partly-checked", ["Backend health per published port", "Traffic in and out of the application", "Dropped packets on backend interfaces"], {
    onByDefault: true, auth: "certificate",
    note: "Runs no process. Incus reports the load balancer's backend health and the network's traffic counters in its network state, and each instance's interface traffic on its metrics endpoint (each cluster member for its own instances, read with a metrics certificate). Apelora reads them; a module never reaches the Incus API. Request counts, status codes and latency need a proxy module behind the gateway.",
  }),
  network: M("platform", "none", "partly-checked", ["Traffic and packet rate on the segment", "Interface errors and drops per instance", "Interface throughput per instance"], {
    onByDefault: true, auth: "certificate",
    note: "Runs no process. Incus reports the network's counters in its network state and every attached instance's interface counters on its metrics endpoint (per cluster member, read with a metrics certificate). There are no counters for traffic the network's rules refused.",
  }),
  opnsense: M("exporter", "text", "partly-checked", ["Interface throughput and errors", "CPU and memory of the firewall", "Uplink loss and latency", "Disk and pool space"], {
    port: 9100, path: "/metrics", separateListener: true,
    enable: "Install the firewall's optional node metrics plugin, switch it on (off by default) and bind it to the internal interface; it serves on 9100.",
    note: `${FREEBSD} ${NO_AUTH} Packet-filter state and rule counters need SNMP or a separate reader of the firewall's API.`,
  }),
  pfsense: M("native", "protocol", "partly-checked", ["Interface traffic and errors", "Packet-filter states against the limit", "Rule and queue counters", "CPU, memory and storage"], {
    port: 161, transport: "udp", auth: "token", separateListener: true,
    enable: "Turn on the built-in SNMP service, set a read community that is not the default and choose its modules (interfaces, packet filter, host resources).",
    note: `${FREEBSD} SNMP is not metrics text, so there is no mon output: a converter runs outside the firewall, or an optional package adds a node metrics agent. The community string is a shared secret sent in clear; allow UDP 161 only from the monitoring module.`,
  }),
  wireguard: M("native", "counters", "checked", ["Seconds since each peer's last handshake", "Bytes received and sent per peer", "Configured versus active peers"], {
    onByDefault: true,
    enable: "Read the per-peer counters inside the instance with the WireGuard tool (needs network-admin rights). A separate exporter process can serve them on 9586, but the common one has not changed since 2023.",
    note: "A handshake older than about three minutes on a peer with keepalive usually means the tunnel is down. In a system container the WireGuard module comes from the host's kernel.",
  }),
  openvpn: M("native", "counters", "partly-checked", ["Connected clients", "Bytes per client", "Session churn", "Age of the status file (daemon alive)"], {
    enable: "Have the server write a machine-readable status file (version 2 or 3) at a fixed interval. A separate exporter process can read it and serve on 9176; the original is archived.",
    note: "The management socket is clear text with at most a password: keep it on localhost or a Unix socket.",
  }),

  // ---- Security & Identity ----
  keycloak: M("native", "text", "checked", ["Request rate and latency by status", "Database pool use", "Logins and failed logins per realm", "JVM heap after collection"], {
    port: 9000, path: "/metrics", separateListener: true, health: { port: 9000, path: "/health/ready" },
    enable: "Turn on the class's Metrics endpoint setting (a build option); health is a separate switch. Both are served on the management port.",
    note: `Since version 25, metrics and health are served only on the management port 9000, no longer on 8080/8443. ${NO_AUTH} It uses HTTPS when the main listener has TLS.`,
  }),
  vault: M("native", "text", "checked", ["Sealed state", "Request latency and volume", "Leadership changes", "Active leases", "Audit device blocking", "Storage latency"], {
    port: 8200, path: "/v1/sys/metrics?format=prometheus", onByDefault: true, localOnly: true, auth: "token", health: { port: 8200, path: "/v1/sys/health" },
    enable: "Bind the listener to the application network (the default is 127.0.0.1), keep the telemetry retention above zero (24 h by default) and give the collector a token that may read sys/metrics, or allow unauthenticated metrics on that listener.",
    note: "Only the active node answers with metrics; standbys redirect. Health answers 200 when active, 429 on a standby and 503 when sealed. HTTPS when the class's Require TLS is on.",
  }),
  authelia: M("native", "text", "partly-checked", ["Requests by status code", "Authorisation decisions", "First- and second-factor results", "Authentication latency"], {
    port: 9959, path: "/metrics", separateListener: true, health: { port: 9091, path: "/api/health" },
    enable: "Turn on telemetry metrics in the configuration (off by default); it listens on 9959 unless another address is set.",
    note: "The verbose health check needs no authentication and can reveal provider details: keep it internal.",
  }),
  authentik: M("native", "text", "partly-checked", ["Request rate and latency per view", "Worker task backlog and failures", "Outpost connectivity", "Login outcomes"], {
    port: 9300, path: "/metrics", onByDefault: true, separateListener: true, health: { port: 9000, path: "/-/health/ready/" },
    enable: "On by default for the server, the worker and outposts; an empty metrics listen setting turns it off.",
    note: `${NO_AUTH} Current documentation gives only the port; the /metrics path comes from older pages.`,
  }),
  pocketid: M("native", "text", "partly-checked", ["Request rate and latency", "Memory and goroutines", "Database errors", "Sign-in outcomes"], {
    port: 9464, path: "/metrics", localOnly: true, separateListener: true, health: { port: 1411, path: "/healthz" },
    enable: "Set OTEL_METRICS_EXPORTER to the pull (scrape) mode and its scrape host to a non-loopback address (default localhost); it serves on 9464.",
    note: "Metric names are those of the generic OpenTelemetry instrumentation; the project does not list them.",
  }),
  openldap: M("native", "protocol", "partly-checked", ["Current and total connections", "Operations started versus completed", "Thread backlog", "Replication freshness"], {
    port: 389, auth: "configurable",
    enable: "Add the monitor database and allow only a monitoring account to read it; the values are operational attributes under cn=Monitor.",
    note: "Read over LDAP with an authenticated bind, not as metrics text, so there is no mon output: a reader uses an ordinary ldap connection. The common translator to metrics text is archived.",
  }),
  zitadel: M("native", "text", "partly-checked", ["API request rate and latency", "Projection lag", "Database pool use", "Readiness during migrations"], {
    port: 8080, path: "/debug/metrics", onByDefault: true, health: { port: 8080, path: "/debug/ready" },
    enable: "Served on the API port by the older metrics setting (on by default); the newer instrumentation block is off by default and offers a scrape mode.",
    note: "Shares the API port: block /debug/ paths at the proxy in front, so only the drawn mon connection reaches them. Which default applies depends on the release's defaults file.",
  }),

  // ---- Load balancing ----
  haproxy: M("native", "text", "checked", ["Servers up or down", "Backend queue", "5xx responses", "Response time", "Sessions against the limit"], {
    port: 8405, portChosen: true, path: "/metrics", auth: "configurable", separateListener: true,
    enable: "Add a metrics frontend that hands /metrics to the built-in exporter service (present in the official images). The port is set in configuration; the class uses 8405.",
    note: "The Statistics page setting is the separate HTML/CSV statistics page. Protect the metrics frontend with TLS or access rules, or keep it on the application network.",
  }),
  traefik: M("native", "text", "checked", ["Requests by entry point, service and code", "Latency", "Backend health", "Open connections", "Configuration reload failures"], {
    port: 8080, path: "/metrics", separateListener: true, health: { port: 8080, path: "/ping" },
    enable: "Turn on metrics in the static configuration; they are served on the internal 'traefik' entry point (8080). The health page needs the ping option.",
    note: "The same entry point carries the dashboard and API when they are on: keep it on the application network.",
  }),
  envoy: M("native", "text", "checked", ["Upstream 5xx", "Upstream latency", "Healthy hosts versus total", "Circuit breaking and timeouts"], {
    port: 9901, portChosen: true, path: "/stats/prometheus", separateListener: true, health: { port: 9901, path: "/ready" },
    enable: "Define the admin listener in the bootstrap (there is no default; the class uses 9901, the port in the project's examples) and limit it to the stats and readiness paths.",
    note: "The admin interface can change state and reveal configuration: never let it beyond the application network.",
  }),
  caddy: M("native", "text", "checked", ["Requests by server, handler and code", "Request latency", "Upstream health", "Certificate renewal problems"], {
    port: 2020, portChosen: true, path: "/metrics", separateListener: true,
    enable: "Add a metrics handler on a site bound to port 2020 (the class's choice) and turn on HTTP request metrics in the global options; process metrics are always on.",
    note: "The admin endpoint (localhost:2019) also serves /metrics, but do not widen it: its API changes configuration and has no authentication.",
  }),
  varnish: M("native", "counters", "partly-checked", ["Hit ratio", "Evictions", "Backend failures", "Dropped sessions", "Worker thread exhaustion"], {
    onByDefault: true,
    enable: "Counters live in shared memory and are read inside the instance with the statistics tool (plain, JSON or XML). A separate exporter process can serve them on 9131; the common one has not changed since 2024.",
    note: "The open-source project was renamed in 2025–26: check package and program names before building a runtime.",
  }),

  // ---- Web & Application ----
  nginx: M("native", "page", "checked", ["Active connections", "Dropped connections (accepted minus handled)", "Request rate", "Reading, writing and waiting connections"], {
    path: "/stub_status", auth: "configurable", separateListener: true,
    enable: "Add a status location (stub_status, built into the official images) on a server block of its own, on a port you choose, and allow only the collector's address. The vendor's exporter process turns the page into metrics text on 9113.",
    note: "The page is plain text, not metrics text, so there is no mon output. Apelora's runtime does not turn it on yet: the image runs unchanged and there is no way to add configuration files. Per-upstream status is only in the commercial edition.",
  }),
  apache: M("native", "page", "checked", ["Busy versus idle workers", "Request rate", "Traffic", "Connection states", "Restarts"], {
    path: "/server-status?auto", auth: "configurable",
    enable: "Load the status module and add a /server-status location limited by address, on the site's port; '?auto' gives machine-readable text. Extended status is on whenever the module is loaded.",
    note: "A status page, not metrics text, so there is no mon output. A separate exporter process can turn it into metrics text on 9117.",
  }),
  nodejs: M("none", "none", "checked", ["Event-loop delay", "Heap used versus limit", "Garbage-collection pauses", "Request rate, latency and errors"], {
    note: "The runtime serves nothing: the application must serve its own metrics (for example with an OpenTelemetry library). Until then only the resource use Incus reports is available.",
  }),
  tomcat: M("exporter", "text", "checked", ["Busy threads versus max threads", "Requests and errors", "Processing time", "Active sessions", "Heap and GC time"], {
    port: 9404, portChosen: true, path: "/metrics", separateListener: true,
    enable: JVM_AGENT("The Manager's status page (?XML=true, with a manager-status user) is the built-in alternative."),
    note: "The Manager's text and JMX interfaces cannot be protected against cross-site requests: keep them off public listeners.",
  }),
  php: M("native", "protocol", "checked", ["Requests waiting for a worker", "Active versus idle processes", "Pool exhaustion", "Slow requests"], {
    separateListener: true,
    enable: "Set a status path (and, optionally, a status listener of its own) in the pool. It answers as plain text, JSON, XML or OpenMetrics text (8.1+), but only over FastCGI, so the web server in front must publish it.",
    note: "Never expose the FastCGI port: it lets the caller run code. A separate exporter process can read the status over FastCGI and serve metrics text on 9253.",
  }),
  python: M("none", "none", "checked", ["Request rate, latency and errors", "Busy workers", "Process CPU and memory", "Garbage collections"], {
    note: "The interpreter serves nothing: the application must serve its own metrics (a client library or OpenTelemetry). Until then only the resource use Incus reports is available.",
  }),
  openjdk: M("exporter", "text", "checked", ["Heap used after collection", "GC time and count", "Thread count and deadlocks", "Metaspace"], {
    port: 9404, portChosen: true, path: "/metrics", separateListener: true,
    enable: JVM_AGENT("Use the class's JVM options setting, or let the application serve its own metrics. JMX is readable locally without setup."),
    note: "Remote JMX is not an HTTP endpoint; if it is turned on, keep its authentication and TLS on.",
  }),
  wordpress: M("none", "none", "partly-checked", ["PHP worker pool saturation", "5xx responses and latency", "Database connections and slow queries", "Scheduled task backlog"], {
    note: "The core serves no metrics; Site Health checks are admin-only checks, not time series. Read the PHP-FPM, web server and database modules instead.",
  }),
  nextcloud: M("native", "text", "partly-checked", ["Users and active users", "Storage use", "Background job backlog", "Maintenance mode"], {
    port: 80, path: "/metrics", onByDefault: true, localOnly: true,
    enable: "Version 33 and later: list the collector's network in openmetrics_allowed_clients (only localhost by default). Earlier versions: the server information app's JSON API with a token.",
    note: "Served on the web port and protected by client address, not by credentials.",
  }),
  gitea: M("native", "text", "checked", ["Object counts (repositories, users, issues)", "Memory", "Database and cache health", "Request latency"], {
    port: 3000, path: "/metrics", auth: "token", health: { port: 3000, path: "/api/healthz" },
    enable: "Set ENABLED = true in the [metrics] section (off by default) and a TOKEN that callers send as a bearer token. Issue counts by label or repository are costly and off by default.",
    note: "Served on the main web port: without a token anyone who reaches the site can read it.",
  }),
  gitlab: M("native", "text", "checked", ["Request rate and latency", "Request queueing", "Background job latency", "Database time", "Unhandled errors"], {
    port: 80, path: "/-/metrics", onByDefault: true, localOnly: true, health: { port: 80, path: "/-/readiness" },
    enable: "On by default in the Linux package; add the collector's address to the monitoring allow-list (localhost only by default).",
    note: "The package also bundles a collector and several exporters; when the database, cache and store outputs go to other modules, read those modules instead.",
  }),
  jenkins: M("exporter", "text", "partly-checked", ["Queue waiting for executors", "Busy versus free executors", "Offline nodes", "Build failures and duration", "Space left in the home directory"], {
    port: 8080, path: "/prometheus/", auth: "configurable",
    enable: "Install the metrics plugin for scraping from the project's plugin site; it serves on the web port (path set in its settings, default /prometheus/) and can require authentication.",
    note: "Runs inside Jenkins, so no extra process. The default of its authentication setting was not confirmed: restrict the path or require a user.",
  }),

  // ---- Databases & cache ----
  postgresql: M("exporter", "text", "checked", ["Connections versus max_connections", "Transactions and rollbacks", "Cache hit ratio", "Replication lag", "Deadlocks and temporary files", "Long-running transactions"], {
    port: 9187, path: "/metrics", auth: "configurable", separateListener: true,
    enable: "Run a separate exporter process inside the instance, connected with a role granted pg_monitor; it serves on 9187. The statistics themselves are always collected and readable with SQL.",
    note: "Statistics views differ between versions (I/O statistics from 16, checkpoint counters moved in 17), so the exporter must match the chosen version.",
  }),
  mariadb: M("exporter", "text", "partly-checked", ["Connections versus max", "Running threads", "Aborted connections", "Slow queries", "Replica lag", "Buffer pool hit ratio"], {
    port: 9104, path: "/metrics", auth: "configurable", separateListener: true,
    enable: "Run a separate exporter process with an account allowed PROCESS, REPLICATION CLIENT and SELECT; it serves on 9104. Status counters are always readable with SQL.",
  }),
  redis: M("exporter", "text", "checked", ["Memory used versus maxmemory", "Evictions", "Hit ratio", "Connected and blocked clients", "Rejected connections", "Last save failed"], {
    port: 9121, path: "/metrics", auth: "configurable", separateListener: true,
    enable: "Run a separate exporter process with an ACL user limited to the commands it needs; it serves on 9121. The INFO command is the source and is always available.",
  }),
  mysql: M("exporter", "text", "partly-checked", ["Connections versus max", "Running threads", "Slow queries", "Replica lag", "Row lock waits", "Buffer pool hit ratio"], {
    port: 9104, path: "/metrics", auth: "configurable", separateListener: true,
    enable: "Run a separate exporter process with an account allowed PROCESS, REPLICATION CLIENT and SELECT; it serves on 9104. Status counters are always readable with SQL.",
    note: "8.4 and 9.x use replica/source wording: the exporter's version must understand it.",
  }),
  mongodb: M("exporter", "text", "checked", ["Connections current versus available", "Operation rates", "Cache dirty and used versus max", "Replication lag and member state", "Assertions"], {
    port: 9216, path: "/metrics", auth: "configurable", separateListener: true,
    enable: "Run a separate exporter process with a user holding clusterMonitor on admin and read on local; it serves on 9216. Server status is the source and is always available.",
  }),
  couchdb: M("native", "text", "checked", ["Requests and errors by status", "Document reads and writes", "Request latency", "Replicator failures", "VM memory and processes"], {
    port: 5984, path: "/_node/_local/_prometheus", onByDefault: true, auth: "basic", health: { port: 5984, path: "/_up" },
    enable: "Always served on the API port to an administrator. An optional listener without authentication exists but binds to 127.0.0.1 by default.",
  }),
  cassandra: M("exporter", "text", "checked", ["Read and write latency", "Timeouts and unavailables", "Thread pool backlog", "Compaction backlog", "Dropped messages", "Hints"], {
    port: 9404, portChosen: true, path: "/metrics", separateListener: true,
    enable: JVM_AGENT("Metrics are always registered and readable locally over JMX (7199, local only by default) or as virtual tables over CQL."),
    note: "Metrics are per node: read every node.",
  }),
  scylladb: M("native", "text", "partly-checked", ["Read and write latency per shard", "Timeouts", "Reactor utilisation", "Cache hit ratio", "Compaction backlog"], {
    port: 9180, path: "/metrics", onByDefault: true, separateListener: true,
    enable: "On by default; its address follows the listen address. Changing the metric prefix breaks the vendor's dashboards.",
    note: `${NO_AUTH} Recent releases use a source-available licence: check it before packaging an image.`,
  }),
  cockroachdb: M("native", "text", "partly-checked", ["Unavailable ranges", "Under-replicated ranges", "Store capacity left", "SQL latency and errors", "Node liveness"], {
    port: 8080, path: "/_status/vars", onByDefault: true, separateListener: true, health: { port: 8080, path: "/health?ready=1" },
    enable: "Always served on the node's HTTP port, separate from SQL (26257); HTTPS on secure clusters.",
    note: "Whether the endpoint needs credentials was not stated in the documentation read: keep it on the application network.",
  }),
  clickhouse: M("native", "text", "partly-checked", ["Running and failed queries", "Parts and merge backlog", "Replication queue and read-only replicas", "Memory against the limit"], {
    port: 9363, path: "/metrics", separateListener: true, health: { port: 8123, path: "/ping" },
    enable: "Add a metrics block to the server configuration with port 9363 and the tables to export (metrics, events, asynchronous metrics, errors).",
    note: `${NO_AUTH} Whether the shipped configuration already has the block was not confirmed.`,
  }),
  influxdb: M("native", "text", "partly-checked", ["Write and query rate and latency", "Query queueing", "Storage write errors", "Task failures"], {
    port: 8086, portBy: { property: "version", ports: { "2.7": 8086, "3.0": 8181 } }, path: "/metrics", onByDefault: true, auth: "configurable", health: { port: 8086, path: "/health" },
    enable: "2.7: on by default on the API port 8086. 3.0: on the API port 8181, and it needs a token unless authorisation is turned off for metrics.",
    note: "Version 3 is a different engine with different metric names; its health check is on 8181.",
  }),
  neo4j: M("native", "text", "unverified", ["Bolt connections running queries", "Active and rolled-back transactions", "Page cache hit ratio", "Heap and GC time", "Checkpoint duration"], {
    port: 2004, localOnly: true, separateListener: true,
    enable: "Turn on the metrics endpoint in the configuration and widen its address (default localhost:2004).",
    note: `The manual marks this as an Enterprise feature; whether the Community edition serves it, and the exact path, are not confirmed. ${NO_AUTH}`,
  }),
  arangodb: M("native", "text", "partly-checked", ["Scheduler queue length", "Client connections", "Responses by status", "Query time", "Write stalls and compaction backlog"], {
    port: 8529, path: "/_admin/metrics", onByDefault: true, auth: "basic",
    enable: "On by default (startup option for the metrics API); per-document read and write metrics are an extra option.",
    note: "The caller needs read access to the database in the path; basic credentials or a bearer token work. Version 4 moves admin paths.",
  }),
  memcached: M("exporter", "text", "checked", ["Hit ratio", "Evictions of valid items", "Memory used versus limit", "Open connections", "Connection limit reached"], {
    port: 9150, path: "/metrics", separateListener: true,
    enable: "Run a separate exporter process pointed at the server; it serves on 9150. The stats command over the cache protocol is always available.",
  }),

  // ---- Messaging & streaming ----
  kafka: M("exporter", "text", "partly-checked", ["Under-replicated partitions (should be 0)", "Offline partitions", "Exactly one active controller", "Request handler idle ratio", "Fenced brokers"], {
    port: 9404, portChosen: true, path: "/metrics", separateListener: true,
    enable: JVM_AGENT("Remote JMX is off by default and, if used, needs authentication."),
    note: "Consumer lag comes from the consumers, not from the broker.",
  }),
  pulsar: M("native", "text", "checked", ["Subscription backlog", "Inbound message rate", "Storage per topic", "Storage and coordination health"], {
    port: 8080, path: "/metrics/", onByDefault: true, auth: "configurable",
    enable: "On by default on the broker's HTTP port; topic-level series can be switched to namespace totals. Authentication on the endpoint is off by default.",
    note: "The storage and coordination services serve their own metrics on port 8000.",
  }),
  nats: M("exporter", "text", "checked", ["Connections", "Slow consumers", "Pending bytes per client", "Route health", "Stream and consumer state"], {
    port: 7777, path: "/metrics", separateListener: true, health: { port: 8222, path: "/healthz" },
    enable: "Set the monitoring port (off until set; 8222 by convention) and run the project's exporter process against it; it serves on 7777.",
    note: "The monitoring port answers JSON without authentication and lists client details: keep it on the application network.",
  }),
  mosquitto: M("native", "protocol", "partly-checked", ["Connected clients", "Messages and bytes in and out", "Subscriptions", "Load averages"], {
    port: 1883, onByDefault: true, auth: "configurable",
    enable: "The broker publishes statistics under the $SYS topics every 10 s; subscribe to $SYS/# explicitly with an account allowed to read them.",
    note: "Read over MQTT, so there is no mon output: a reader connects like any other client. A separate exporter process exists but is little maintained.",
  }),
  activemq: M("exporter", "text", "unverified", ["Queue depth", "Consumers per queue", "Enqueue and dequeue rates", "Memory and store use", "Expired and dead messages"], {
    port: 9404, portChosen: true, path: "/metrics", separateListener: true,
    enable: JVM_AGENT("Both flavours publish JMX inside the JVM; Artemis can also load a metrics plugin."),
    note: "Neither flavour has a built-in metrics-text endpoint; the web console's JMX bridge paths were not confirmed.",
  }),
  rabbitmq: M("native", "text", "checked", ["Ready messages per queue", "Unacknowledged messages", "Connection churn", "Memory versus high watermark", "Free disk versus alarm limit"], {
    port: 15692, path: "/metrics", separateListener: true,
    enable: "Enable the bundled metrics plugin on every node (already on in the official images, not in distribution packages); it serves on 15692.",
    note: `/metrics is aggregated; per-object detail is on /metrics/per-object and /metrics/detailed. Authentication is off unless turned on. ${NO_AUTH}`,
  }),

  // ---- Search & analytics ----
  elasticsearch: M("exporter", "text", "partly-checked", ["Cluster status", "Unassigned shards", "Heap pressure per node", "Rejected thread-pool tasks", "Disk against watermarks"], {
    port: 9114, path: "/metrics", auth: "basic", separateListener: true, health: { port: 9200, path: "/_cluster/health" },
    enable: "Run a separate exporter process with a monitoring user or API key; it serves on 9114 and reads node and index statistics on every request.",
    note: "TLS and authentication are on by default since 8.0. Short read intervals load the master nodes.",
  }),
  opensearch: M("exporter", "text", "partly-checked", ["Cluster status", "Heap and GC time", "Indexing throttled", "Thread-pool rejections", "Disk use"], {
    port: 9200, path: "/_prometheus/metrics", auth: "basic", health: { port: 9200, path: "/_cluster/health" },
    enable: "Install the project's metrics plugin on every node, built for the exact version; it adds the path to the HTTP port.",
    note: "With the security plugin (on by default) the path needs TLS and credentials.",
  }),
  solr: M("native", "text", "partly-checked", ["Request rate and latency per handler", "Responses by status", "Live nodes", "Index size", "Heap and GC"], {
    port: 8983, portBy: { property: "version", ports: { "9.6": 8989, "9.8": 8983 } }, path: "/solr/admin/metrics?wt=prometheus", onByDefault: true, auth: "configurable",
    enable: "9.8: the metrics API answers as metrics text when asked. 9.6: run the exporter tool bundled with the full distribution and set its port (the class uses 8989).",
    note: "Authentication depends on whether an authentication plugin is configured (none by default).",
  }),
  kibana: M("native", "json", "partly-checked", ["Overall status", "Elasticsearch connection", "Queued requests to Elasticsearch", "Plugin status"], {
    port: 5601, path: "/api/status", onByDefault: true, auth: "basic", health: { port: 5601, path: "/api/status" },
    enable: "Always on. Without credentials it answers only the overall level, which is enough for a liveness check.",
    note: "JSON, so there is no mon output. A metrics-text route exists through a monitoring plugin setting, documented only for 8.12.",
  }),
  logstash: M("native", "json", "partly-checked", ["Health report status", "Events in, filtered and out per pipeline", "Persistent queue backlog", "Worker utilisation", "Heap"], {
    port: 9600, path: "/_node/stats", onByDefault: true, localOnly: true, auth: "configurable", separateListener: true, health: { port: 9600, path: "/_health_report" },
    enable: "On by default but bound to 127.0.0.1 on the first free port in 9600–9700: set the API host to the application network address and pin the port.",
    note: "JSON only, so there is no mon output. Authentication and TLS are off by default; with both set, the API binds to every interface.",
  }),
  spark: M("native", "text", "checked", ["Failed tasks and jobs", "Running and failed stages", "Executor heap and GC", "Shuffle bytes", "Block manager memory"], {
    port: 4040, path: "/metrics/executors/prometheus/",
    enable: "Executor metrics on the driver UI need spark.ui.prometheus.enabled (on by default in 4.0, off in 3.5); the experimental metrics-text sink must be configured in both.",
    note: "The driver UI exists only while an application runs; a standalone master serves on 8080 and workers on 8081. No authentication unless access lists are configured: keep the UI ports on the application network.",
  }),
  airflow: M("native", "protocol", "partly-checked", ["Scheduler alive", "Task failures", "Run duration", "Free pool slots", "Queued tasks"], {
    health: { port: 8974, path: "/health" },
    enable: "Airflow pushes metrics instead of serving them: turn on one of its push outputs in the [metrics] section (otel_on, or the older statsd_on) and point it at a collector (the OTel Collector module can receive them).",
    note: "Pushes, so there is no mon output. The scheduler's health server (8974) is off by default.",
  }),

  // ---- Storage ----
  minio: M("native", "text", "checked", ["Read and write quorum", "Drives and nodes offline", "Capacity used versus free", "S3 errors and latency"], {
    port: 9000, path: "/minio/v2/metrics/cluster", onByDefault: true, auth: "token", health: { port: 9000, path: "/minio/health/live" },
    enable: "Always on on the S3 port; the collector needs a bearer token generated with the admin client, unless public metrics are allowed.",
    note: "Health endpoints need no credentials. The community repository is archived and no longer maintained: a catalog risk.",
  }),
  ceph: M("native", "text", "partly-checked", ["Cluster health", "Capacity per storage daemon", "Pool use", "Daemons reachable"], {
    port: 9283, path: "/metrics", separateListener: true,
    enable: "Enable the manager's metrics module (9283, 9284 with TLS); the container-based deployment tool turns the monitoring stack on by default.",
    note: `Only the active manager serves real data. ${NO_AUTH} The module ignores the path.`,
  }),
  nfs: M("exporter", "text", "partly-checked", ["Requests by operation", "RPC errors", "All server threads busy", "Bytes read and written", "Export free space"], {
    port: 9100, path: "/metrics", separateListener: true,
    enable: "Run the node metrics agent inside the instance; its NFS server collector reads the kernel's counters and is on by default.",
    note: "The kernel NFS server is restricted in unprivileged containers, so this class will probably need a virtual machine.",
  }),

  // ---- Monitoring ----
  prometheus: M("native", "text", "checked", ["Ready to serve queries", "Active series", "Failed rule evaluations", "Dropped alert notifications", "Failed compactions"], {
    port: 9090, path: "/metrics", onByDefault: true, auth: "configurable", health: { port: 9090, path: "/-/ready" },
    enable: "Always on on the web port.",
    note: "Its scrape input reads other modules' mon outputs; its own mon output lets another collector read it. Authentication and TLS are off unless a web configuration file adds them.",
  }),
  grafana: M("native", "text", "checked", ["Database reachable", "Request latency and errors", "Alert rule evaluation failures", "Data source errors"], {
    port: 3000, path: "/metrics", onByDefault: true, auth: "configurable", health: { port: 3000, path: "/api/health" },
    enable: "On by default on the web port; set metrics basic-auth credentials to protect it.",
  }),
  jaeger: M("native", "text", "checked", ["Spans accepted and refused", "Spans written to storage", "Storage write failures", "Accepted versus sent (backlog)"], {
    port: 8888, path: "/metrics", onByDefault: true, separateListener: true, health: { port: 13133, path: "/status" },
    enable: "On by default in version 2 on 8888; the health extension serves on 13133.",
    note: `Version 1 has reached end of life; use version 2. ${NO_AUTH}`,
  }),
  otel: M("native", "text", "checked", ["Data accepted and refused by receivers", "Export failures", "Exporter queue fill", "Items dropped"], {
    port: 8888, path: "/metrics", onByDefault: true, localOnly: true, separateListener: true, health: { port: 13133, path: "/" },
    enable: "On by default on 127.0.0.1:8888; add a pull reader for its own telemetry with a non-loopback host so a collector can read it.",
    note: `The health extension runs only when it is listed in the service's extensions. ${NO_AUTH}`,
  }),

  // ---- Operating systems ----
  "linux-base": M("agent", "text", "checked", OS_SIGNALS, { port: 9100, path: "/metrics", separateListener: true, enable: LINUX_AGENT, note: INCUS_VM }),
  debian: M("agent", "text", "partly-checked", OS_SIGNALS, { port: 9100, path: "/metrics", separateListener: true, enable: LINUX_AGENT, note: INCUS_CT }),
  ubuntu: M("agent", "text", "partly-checked", OS_SIGNALS, { port: 9100, path: "/metrics", separateListener: true, enable: LINUX_AGENT, note: INCUS_CT }),
  alpine: M("agent", "text", "partly-checked", OS_SIGNALS, { port: 9100, path: "/metrics", separateListener: true, enable: `${LINUX_AGENT} The package is in the community repository.`, note: INCUS_CT }),
  rocky: M("agent", "text", "partly-checked", OS_SIGNALS, { port: 44322, path: "/metrics", auth: "configurable", separateListener: true, enable: RHEL_AGENT, note: INCUS_CT }),
  almalinux: M("agent", "text", "partly-checked", OS_SIGNALS, { port: 44322, path: "/metrics", auth: "configurable", separateListener: true, enable: RHEL_AGENT, note: INCUS_CT }),
  fedora: M("agent", "text", "unverified", OS_SIGNALS, { port: 44322, path: "/metrics", auth: "configurable", separateListener: true, enable: RHEL_AGENT, note: INCUS_CT }),

  // ---- Microsoft Windows ----
  "windows-server": M("agent", "text", "partly-checked", ["CPU use", "Available memory", "Disk latency and free space", "Stopped automatic services", "Network throughput"], { port: 9182, path: "/metrics", auth: "configurable", separateListener: true, enable: WIN_DESKTOP, note: WIN_VM }),
  "windows-11": M("agent", "text", "partly-checked", ["CPU use", "Available memory", "Disk latency and free space", "Interactive sessions", "Network throughput"], { port: 9182, path: "/metrics", auth: "configurable", separateListener: true, enable: WIN_DESKTOP, note: WIN_VM }),
  "windows-10": M("agent", "text", "partly-checked", ["CPU use", "Available memory", "Disk latency and free space", "Network throughput"], { port: 9182, path: "/metrics", auth: "configurable", separateListener: true, enable: WIN_DESKTOP, note: WIN_VM }),
  "active-directory": M("exporter", "text", "partly-checked", ["Inbound replication", "LDAP searches and binds", "Request queue backlog", "Authentication bottlenecks", "Database disk latency"], {
    port: 9182, path: "/metrics", auth: "configurable", separateListener: true, enable: WIN_EXPORTER("directory (ad)"), note: WIN_VM,
  }),
  iis: M("exporter", "text", "partly-checked", ["Requests per site", "Current connections", "Stopped application pools", "Requests queued in the kernel", "Error responses"], {
    port: 9182, path: "/metrics", auth: "configurable", separateListener: true, enable: WIN_EXPORTER("web server (iis)"), note: WIN_VM,
  }),
  "sql-server": M("exporter", "text", "checked", ["Page life expectancy", "Batch requests per second", "User connections", "Deadlocks and lock timeouts", "Log space", "Availability group traffic"], {
    port: 9182, path: "/metrics", auth: "configurable", separateListener: true, enable: WIN_EXPORTER("database (mssql)"),
    note: `The same values are readable in T-SQL from the performance counter view. ${WIN_VM}`,
  }),
  dotnet: M("none", "none", "partly-checked", ["Request duration and status codes", "In-flight requests", "Queued connections", "Unhandled exceptions", "GC and thread pool"], {
    enable: "The application must opt in: add the OpenTelemetry library with its scrape endpoint, which serves /metrics on the application's own port.",
    note: "Until the application serves metrics, only the resource use Incus reports is available.",
  }),
  "smb-files": M("exporter", "text", "partly-checked", ["Open files per share", "Connected clients", "Throughput", "Request rates", "Free space against quota"], {
    port: 9182, path: "/metrics", auth: "configurable", separateListener: true, enable: WIN_EXPORTER("file sharing (smb)"), note: WIN_VM,
  }),
  "rd-gateway": M("exporter", "text", "unverified", ["Current connections", "Failed versus successful connections", "Authorisation time", "Web front-end health"], {
    port: 9182, path: "/metrics", auth: "configurable", separateListener: true,
    enable: "Install a Windows metrics exporter service and configure its generic performance-counter collector to read the gateway's counter object; it serves on 9182.",
    note: `Only the gateway's counter object name was confirmed; check counter names on a real gateway. ${WIN_VM}`,
  }),
  exchange: M("exporter", "text", "partly-checked", ["Health state per component", "Client RPC latency", "Mail in retry queues", "Directory read latency", "Web and mobile request rates"], {
    port: 9182, path: "/metrics", auth: "configurable", separateListener: true, enable: WIN_EXPORTER("mail (exchange)"),
    note: `Exchange's own health sets are read with Get-ServerHealth in its management shell. ${WIN_VM}`,
  }),

  // ---- Utilities ----
  consul: M("native", "text", "checked", ["Autopilot healthy", "Leader contact time", "Raft commit time", "Raft thread saturation", "Certificate expiry"], {
    port: 8500, path: "/v1/agent/metrics?format=prometheus", auth: "token", health: { port: 8500, path: "/v1/status/leader" },
    enable: "Set the telemetry retention for metrics text above zero (0 by default) and, when ACLs are on, give the collector a token with agent:read.",
    note: "Under a source-available licence since 2023: a catalog licensing point.",
  }),
  etcd: M("native", "text", "checked", ["Member has a leader", "Leader changes", "Failed and pending proposals", "WAL sync latency", "Database size versus quota"], {
    port: 2379, path: "/metrics", onByDefault: true, localOnly: true, auth: "configurable", health: { port: 2379, path: "/health" },
    enable: "Always served on the client listener, which binds to localhost by default: set the client URLs (or a separate metrics listener) to the application network address.",
    note: "Needs a client certificate only when the client listener requires them.",
  }),
};
