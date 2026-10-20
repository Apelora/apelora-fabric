import { parse } from "yaml";
import { addModule, addPort, connect, createEmptyAssembly, explainConnection, renameModule, setConfig, slug, tidyLayout, validateProperty, type ModuleClass, type ApplicationAssembly, type Port } from "@apelora/assembly-model";
import { lookupClass } from "./classes";

export type ComposeResult = { ok: true; assembly: ApplicationAssembly; notes: string[] } | { ok: false; errors: string[] };

const MAX_BYTES = 200_000;
const MAX_SERVICES = 60;

/** Container image name (without registry, namespace or tag) → catalog class. Anything unknown becomes a Linux base module. */
const IMAGE_CLASS: Readonly<Record<string, string>> = {
  postgres: "postgresql", postgresql: "postgresql", pgbouncer: "postgresql", timescaledb: "postgresql",
  mysql: "mysql", mariadb: "mariadb", mongo: "mongodb", mongodb: "mongodb", redis: "redis", valkey: "redis", memcached: "memcached",
  couchdb: "couchdb", cassandra: "cassandra", scylla: "scylladb", cockroach: "cockroachdb", clickhouse: "clickhouse", influxdb: "influxdb", neo4j: "neo4j", arangodb: "arangodb",
  nginx: "nginx", httpd: "apache", apache: "apache", traefik: "traefik", haproxy: "haproxy", caddy: "caddy", envoy: "envoy", varnish: "varnish",
  node: "nodejs", python: "python", php: "php", tomcat: "tomcat", openjdk: "openjdk", "eclipse-temurin": "openjdk", wordpress: "wordpress", nextcloud: "nextcloud", gitea: "gitea", gitlab: "gitlab", jenkins: "jenkins",
  rabbitmq: "rabbitmq", kafka: "kafka", pulsar: "pulsar", nats: "nats", mosquitto: "mosquitto", activemq: "activemq",
  elasticsearch: "elasticsearch", opensearch: "opensearch", kibana: "kibana", logstash: "logstash", solr: "solr",
  minio: "minio", prometheus: "prometheus", grafana: "grafana", jaeger: "jaeger", consul: "consul", etcd: "etcd", vault: "vault",
  keycloak: "keycloak", authelia: "authelia", authentik: "authentik", openldap: "openldap", zitadel: "zitadel",
  debian: "debian", ubuntu: "ubuntu", alpine: "alpine", fedora: "fedora", rockylinux: "rocky", almalinux: "almalinux",
};

/** Environment variable stems (after the product prefix) that name a catalog setting. */
const ENV_ALIAS: Readonly<Record<string, string>> = { db: "database", dbname: "database", name: "database" };

function imageName(image: string): string {
  const noTag = image.split("@")[0]!.replace(/:[^/:]*$/, "");
  const last = noTag.split("/").pop() ?? noTag;
  return last.toLowerCase();
}

function dependsOn(v: unknown): string[] {
  if (Array.isArray(v)) return v.filter((x): x is string => typeof x === "string");
  if (v && typeof v === "object") return Object.keys(v);
  return [];
}

/** Picks the pair of terminals to cable for "A depends on B": A's output to B's input, exact protocol first. */
function pickPair(a: ApplicationAssembly, fromId: string, toId: string): { sp: Port; tp: Port } | null {
  const from = a.spec.modules.find((x) => x.id === fromId);
  const to = a.spec.modules.find((x) => x.id === toId);
  if (!from || !to) return null;
  let best: { sp: Port; tp: Port; score: number } | null = null;
  for (const sp of from.ports) {
    if (sp.direction !== "output" || sp.type === "network" || sp.type === "metrics") continue;
    for (const tp of to.ports) {
      if (tp.direction !== "input" || tp.type === "network") continue;
      if (explainConnection(a, { moduleId: fromId, portId: sp.id }, { moduleId: toId, portId: tp.id })) continue;
      const score = (sp.type === tp.type ? 2 : 0) + (tp.optional ? 0 : 1);
      if (!best || score > best.score) best = { sp, tp, score };
    }
  }
  return best;
}

/**
 * Turns a Compose file into an assembly: each service becomes a module of the matching catalog class
 * (otherwise a Linux base module), each `depends_on` becomes a cable where a compatible pair of terminals
 * exists, and the result is laid out left to right. The input is untrusted: size, service count and shape are checked,
 * and only a few plain fields are read. Nothing is executed or fetched.
 */
export function importCompose(text: string): ComposeResult {
  if (text.length > MAX_BYTES) return { ok: false, errors: [`File is larger than ${MAX_BYTES / 1000} kB.`] };
  let doc: unknown;
  try {
    doc = parse(text, { maxAliasCount: 20, schema: "core" });
  } catch (e) {
    return { ok: false, errors: [`Not valid YAML: ${(e as Error).message.split("\n")[0]}`] };
  }
  const services = doc && typeof doc === "object" ? (doc as { services?: unknown }).services : undefined;
  if (!services || typeof services !== "object" || Array.isArray(services)) return { ok: false, errors: ["No “services:” section found. Is this a Compose file?"] };
  const entries = Object.entries(services as Record<string, unknown>);
  if (entries.length === 0) return { ok: false, errors: ["The “services:” section is empty."] };
  if (entries.length > MAX_SERVICES) return { ok: false, errors: [`More than ${MAX_SERVICES} services; split the file first.`] };

  const notes: string[] = [];
  let a: ApplicationAssembly = createEmptyAssembly("imported-compose");
  const byService = new Map<string, string>();
  /** Services whose image is unknown: their terminals are created on demand to match what they depend on. */
  const generic = new Set<string>();
  entries.forEach(([service, def], i) => {
    const d = def && typeof def === "object" ? (def as Record<string, unknown>) : {};
    const image = typeof d.image === "string" ? d.image : "";
    const classId = IMAGE_CLASS[imageName(image)];
    const cls: ModuleClass = lookupClass(classId ?? "linux-base", "1.0.0")!;
    if (!classId) generic.add(service);
    if (!classId) notes.push(`${service}: ${image ? `image “${image.slice(0, 60)}” is not in the catalog` : "no image given"}; added as a Linux base module.`);
    const r = addModule(a, cls, { x: (i % 6) * 220, y: Math.floor(i / 6) * 160 });
    a = renameModule(r.assembly, r.id, service.slice(0, 48));
    byService.set(service, r.id);
  });

  let published = 0;
  for (const [service, def] of entries) {
    const d = def && typeof def === "object" ? (def as Record<string, unknown>) : {};
    const from = byService.get(service)!;
    for (const dep of dependsOn(d.depends_on)) {
      const to = byService.get(dep);
      if (!to) { notes.push(`${service}: depends on “${dep}”, which is not a service in this file.`); continue; }
      let pair = pickPair(a, from, to);
      if (!pair && (generic.has(service) || generic.has(dep))) {
        const host = generic.has(service) ? from : to;
        const other = a.spec.modules.find((x) => x.id === (host === from ? to : from))!;
        const want = other.ports.filter((q) => q.direction === (host === from ? "input" : "output") && q.type !== "network" && q.type !== "metrics").sort((q, w) => Number(q.optional) - Number(w.optional))[0];
        if (want) { a = addPort(a, host, host === from ? "output" : "input", want.type).assembly; pair = pickPair(a, from, to); }
      }
      if (!pair) { notes.push(`${service} → ${dep}: no compatible terminals, so no cable was drawn.`); continue; }
      const r = connect(a, { moduleId: from, portId: pair.sp.id }, { moduleId: to, portId: pair.tp.id });
      if ("error" in r) { notes.push(`${service} → ${dep}: ${r.error.message}`); continue; }
      a = r.assembly;
    }
    // A few well-known settings carry over when the class has a matching field.
    const cls = lookupClass(a.spec.modules.find((x) => x.id === from)!.classRef.name, "1.0.0");
    const env = d.environment;
    const pairs: [string, string][] = Array.isArray(env)
      ? env.filter((x): x is string => typeof x === "string").map((x) => { const i = x.indexOf("="); return i > 0 ? [x.slice(0, i), x.slice(i + 1)] as [string, string] : [x, ""] as [string, string]; })
      : env && typeof env === "object" ? Object.entries(env as Record<string, unknown>).map(([k, v]) => [k, String(v ?? "")] as [string, string]) : [];
    for (const [k, v] of pairs) {
      const key = slug(k.replace(/^[A-Z]+_/, ""));
      const wanted = ENV_ALIAS[key] ?? key;
      const prop = cls?.properties?.find((q) => q.type !== "secret" && slug(q.id) === wanted);
      if (!prop || !v || v.length > 200) continue;
      const value = prop.type === "int" ? Number(v) : prop.type === "bool" ? v === "true" : v;
      if (!validateProperty(prop, value)) a = setConfig(a, from, prop.id, value);
    }
    if (d.ports) published++;
  }
  if (published) notes.push(`${published} service${published > 1 ? "s publish" : " publishes"} ports; published ports are not modelled, so use a Gateway module for public access.`);
  a = { ...a, metadata: { ...a.metadata, name: "imported-compose" } };
  return { ok: true, assembly: tidyLayout(a), notes: notes.slice(0, 40) };
}
