import { addModule, connect, createEmptyAssembly, renameModule, tagPort, tidyLayout, type ApplicationAssembly } from "@apelora/assembly-model";
import { lookupClass } from "./classes";

export interface Template {
  id: string;
  title: string;
  summary: string;
  /** Short facts shown on the card: what it demonstrates. */
  tags: readonly string[];
  build: () => ApplicationAssembly;
}

type Node = readonly [key: string, classId: string, name?: string];
type Cable = readonly [from: string, fromPort: string, to: string, toPort: string];
type Tag = readonly [key: string, port: string, tag: string];

/** Builds a valid assembly from keys: modules are created, cabled, tagged and then laid out left to right. */
function blueprint(name: string, nodes: readonly Node[], cables: readonly Cable[], tags: readonly Tag[] = []): ApplicationAssembly {
  let a: ApplicationAssembly = { ...createEmptyAssembly(name), metadata: { name, uid: `assembly-template-${name}` } };
  const ids = new Map<string, string>();
  nodes.forEach(([key, classId, label], i) => {
    const cls = lookupClass(classId, "1.0.0");
    if (!cls) throw new Error(`template ${name}: unknown class ${classId}`);
    const r = addModule(a, cls, { x: (i % 6) * 220, y: Math.floor(i / 6) * 160 });
    a = r.assembly;
    if (label) a = renameModule(a, r.id, label);
    ids.set(key, r.id);
  });
  const id = (k: string): string => { const v = ids.get(k); if (!v) throw new Error(`template ${name}: unknown key ${k}`); return v; };
  for (const [s, sp, t, tp] of cables) {
    const r = connect(a, { moduleId: id(s), portId: sp }, { moduleId: id(t), portId: tp });
    if ("error" in r) throw new Error(`template ${name}: ${s}.${sp} → ${t}.${tp}: ${r.error.message}`);
    a = r.assembly;
  }
  for (const [k, port, tag] of tags) {
    const r = tagPort(a, { moduleId: id(k), portId: port }, tag);
    if ("error" in r) throw new Error(`template ${name}: tag ${tag}: ${r.error.message}`);
    a = r.assembly;
  }
  return tidyLayout(a);
}

const nicsOnto = (net: string, keys: readonly string[], tag: string): Tag[] => [[net, "net", tag], ...keys.map((k) => [k, "net", tag] as const)];

export const TEMPLATES: readonly Template[] = [
  {
    id: "ha-web-stack",
    title: "Highly available web stack",
    summary: "Gateway, load balancer, three application servers, PostgreSQL, Redis, and a Prometheus + Grafana pair watching the database.",
    tags: ["Load balancing", "Cache", "Monitoring"],
    build: () => blueprint("ha-web-stack",
      [["gw", "gateway", "edge"], ["lb", "haproxy", "balancer"], ["app1", "nodejs", "app-1"], ["app2", "nodejs", "app-2"], ["app3", "nodejs", "app-3"], ["db", "postgresql", "database"], ["cache", "redis", "cache"], ["prom", "prometheus", "metrics"], ["graf", "grafana", "dashboards"]],
      [["gw", "lan", "lb", "frontend"], ["lb", "backend", "app1", "http"], ["lb", "backend", "app2", "http"], ["lb", "backend", "app3", "http"],
        ["app1", "db", "db", "postgres"], ["app2", "db", "db", "postgres"], ["app3", "db", "db", "postgres"],
        ["app1", "cache", "cache", "redis"], ["app2", "cache", "cache", "redis"], ["app3", "cache", "cache", "redis"],
        ["db", "metrics", "prom", "scrape"], ["graf", "query", "prom", "api"], ["graf", "source", "db", "postgres"]]),
  },
  {
    id: "event-pipeline",
    title: "Event streaming pipeline",
    summary: "Two producers and a consumer talk to Kafka; the consumer stores results in PostgreSQL and Prometheus scrapes the broker.",
    tags: ["Kafka", "Messaging", "Monitoring"],
    build: () => blueprint("event-pipeline",
      [["p1", "openjdk", "producer-1"], ["p2", "openjdk", "producer-2"], ["c1", "openjdk", "consumer"], ["k", "kafka", "broker"], ["db", "postgresql", "results"], ["prom", "prometheus", "metrics"], ["lb", "haproxy", "ingest"], ["gw", "gateway", "edge"]],
      [["gw", "lan", "lb", "frontend"], ["lb", "backend", "p1", "http"], ["lb", "backend", "p2", "http"], ["lb", "backend", "c1", "http"],
        ["p1", "mq", "k", "in"], ["p2", "mq", "k", "in"], ["c1", "mq", "k", "in"], ["c1", "db", "db", "postgres"], ["k", "mon", "prom", "scrape"], ["db", "metrics", "prom", "scrape"]]),
  },
  {
    id: "log-pipeline",
    title: "Search and log pipeline",
    summary: "Vault audit logs flow through Logstash into Elasticsearch, with Kibana for search and Vault itself behind a proxy, and Prometheus on the cluster.",
    tags: ["Elasticsearch", "Logging", "Security"],
    build: () => blueprint("log-pipeline",
      [["v", "vault", "secrets"], ["ls", "logstash", "ingest"], ["es", "elasticsearch", "search"], ["kb", "kibana", "explore"], ["px", "traefik", "proxy"], ["gw", "gateway", "edge"], ["prom", "prometheus", "metrics"]],
      [["v", "audit", "ls", "in"], ["ls", "out", "es", "http"], ["kb", "es", "es", "http"], ["gw", "lan", "px", "in"], ["px", "out", "kb", "ui"], ["px", "out", "v", "api"], ["es", "mon", "prom", "scrape"], ["v", "mon", "prom", "scrape"]]),
  },
  {
    id: "mongo-replica-set",
    title: "MongoDB replica set",
    summary: "Three MongoDB members replicating in a ring, monitored by Prometheus and graphed in Grafana.",
    tags: ["MongoDB", "Replication", "Monitoring"],
    build: () => blueprint("mongo-replica-set",
      [["m1", "mongodb", "member-1"], ["m2", "mongodb", "member-2"], ["m3", "mongodb", "member-3"], ["prom", "prometheus", "metrics"], ["graf", "grafana", "dashboards"]],
      [["m1", "repl", "m2", "mongo"], ["m2", "repl", "m3", "mongo"], ["m3", "repl", "m1", "mongo"], ["m1", "mon", "prom", "scrape"], ["m2", "mon", "prom", "scrape"], ["m3", "mon", "prom", "scrape"], ["graf", "query", "prom", "api"]]),
  },
  {
    id: "sso-keycloak",
    title: "Single sign-on with Keycloak",
    summary: "Gateway and Traefik in front of Keycloak, backed by PostgreSQL, with metrics scraped by Prometheus.",
    tags: ["Identity", "OIDC", "PostgreSQL"],
    build: () => blueprint("sso-keycloak",
      [["gw", "gateway", "edge"], ["px", "traefik", "proxy"], ["kc", "keycloak", "identity"], ["db", "postgresql", "identity-db"], ["prom", "prometheus", "metrics"]],
      [["gw", "lan", "px", "in"], ["px", "out", "kc", "http"], ["kc", "db", "db", "postgres"], ["kc", "mon", "prom", "scrape"], ["db", "metrics", "prom", "scrape"]]),
  },
  {
    id: "sso-authelia",
    title: "Forward-auth gateway with Authelia",
    summary: "Authelia checks users against OpenLDAP and keeps sessions in Redis; Caddy fronts it.",
    tags: ["Identity", "LDAP", "Sessions"],
    build: () => blueprint("sso-authelia",
      [["gw", "gateway", "edge"], ["web", "caddy", "web"], ["au", "authelia", "auth"], ["ldap", "openldap", "directory"], ["redis", "redis", "sessions"]],
      [["gw", "lan", "web", "in"], ["web", "out", "au", "http"], ["au", "ldap", "ldap", "ldap"], ["au", "cache", "redis", "redis"]]),
  },
  {
    id: "wordpress-cache",
    title: "WordPress with caching",
    summary: "Varnish in front of WordPress, using MariaDB for content and Memcached for objects.",
    tags: ["CMS", "Cache", "MySQL"],
    build: () => blueprint("wordpress-cache",
      [["gw", "gateway", "edge"], ["cache", "varnish", "page-cache"], ["wp", "wordpress", "site"], ["db", "mariadb", "content"], ["mc", "memcached", "object-cache"]],
      [["gw", "lan", "cache", "in"], ["cache", "origin", "wp", "http"], ["wp", "db", "db", "mysql"], ["wp", "cache", "mc", "mc"]]),
  },
  {
    id: "windows-domain",
    title: "Windows domain",
    summary: "Active Directory serving IIS and a remote-desktop gateway to two desktops, with SQL Server behind IIS, all on one corporate network joined by a link tag.",
    tags: ["Windows", "Active Directory", "Link tags"],
    build: () => {
      const keys = ["ad", "iis", "sql", "rdg", "pc1", "pc2"];
      return blueprint("windows-domain",
        [["gw", "gateway", "edge"], ["net", "network", "corp-net"], ["ad", "active-directory", "domain"], ["iis", "iis", "intranet"], ["sql", "sql-server", "database"], ["rdg", "rd-gateway", "remote-gw"], ["pc1", "windows-11", "desk-1"], ["pc2", "windows-11", "desk-2"]],
        [["gw", "lan", "rdg", "https"], ["gw", "lan", "iis", "http"], ["iis", "db", "sql", "sql"], ["iis", "auth", "ad", "ldap"], ["rdg", "auth", "ad", "ldap"], ["rdg", "rdp", "pc1", "rdp"], ["rdg", "rdp", "pc2", "rdp"]],
        nicsOnto("net", keys, "corp"));
    },
  },
  {
    id: "monitoring-stack",
    title: "Monitoring stack",
    summary: "A small web tier whose proxy and database are scraped by Prometheus, with Grafana on top.",
    tags: ["Metrics", "Dashboards"],
    build: () => blueprint("monitoring-stack",
      [["gw", "gateway", "edge"], ["px", "traefik", "proxy"], ["app", "nodejs", "app"], ["pg", "postgresql", "database"], ["prom", "prometheus", "metrics"], ["graf", "grafana", "dashboards"]],
      [["gw", "lan", "px", "in"], ["px", "out", "app", "http"], ["app", "db", "pg", "postgres"], ["px", "mon", "prom", "scrape"], ["pg", "metrics", "prom", "scrape"], ["graf", "query", "prom", "api"], ["graf", "source", "pg", "postgres"]]),
  },
];
