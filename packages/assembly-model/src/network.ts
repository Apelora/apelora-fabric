import type { ApplicationAssembly, Endpoint, ModuleInstance, Port, PortType } from "./schema";
import { metricsEndpoint } from "./metrics";
import type { ClassLookup } from "./validate";

/**
 * Network intents: what a design's connections allow, compiled deterministically from the
 * graph (Phase 07, see the `fabric-networking` skill). A connection is a dependency plus an
 * authorisation: the consumer (an output terminal) may reach the provider (an input terminal)
 * on that protocol, addressed by the provider's stable service name. Anything not connected
 * is denied. `network` terminals are NICs: they join L2 segments, not services.
 *
 * Metrics are read, not sent: a `metrics` output (`mon`) is an endpoint, so the module at the
 * other end (a collector's `scrape` input) reaches the module that serves them, on the port its
 * class declares (`ModuleClass.metrics`, docs/architecture/MODULE-PORTS.md).
 */

/** Well-known listening port per protocol. `tcp` is generic and has no single port. */
export const DEFAULT_PORTS: Readonly<Partial<Record<PortType, { transport: "tcp" | "udp"; port: number }>>> = {
  http: { transport: "tcp", port: 80 },
  grpc: { transport: "tcp", port: 50051 },
  postgres: { transport: "tcp", port: 5432 },
  mysql: { transport: "tcp", port: 3306 },
  mssql: { transport: "tcp", port: 1433 },
  mongodb: { transport: "tcp", port: 27017 },
  cassandra: { transport: "tcp", port: 9042 },
  redis: { transport: "tcp", port: 6379 },
  memcached: { transport: "tcp", port: 11211 },
  amqp: { transport: "tcp", port: 5672 },
  kafka: { transport: "tcp", port: 9092 },
  pulsar: { transport: "tcp", port: 6650 },
  mqtt: { transport: "tcp", port: 1883 },
  nats: { transport: "tcp", port: 4222 },
  s3: { transport: "tcp", port: 9000 },
  // Only for classes that declare no metrics port (custom classes): the class's own port wins.
  metrics: { transport: "tcp", port: 9100 },
  ldap: { transport: "tcp", port: 389 },
  smtp: { transport: "tcp", port: 25 },
  syslog: { transport: "udp", port: 514 },
  nfs: { transport: "tcp", port: 2049 },
  smb: { transport: "tcp", port: 445 },
  rdp: { transport: "tcp", port: 3389 },
};

export interface ServiceEndpoint {
  port: string;
  protocol: PortType;
  transport: "tcp" | "udp";
  /** `null` when the protocol has no well-known port (generic `tcp`). */
  number: number | null;
}

export interface Service {
  module: string;
  /** Stable name consumers use, independent of instances and addresses. */
  name: string;
  endpoints: ServiceEndpoint[];
}

export interface NetworkIntent {
  /** Deterministic id: `<from>.<port>-><to>.<port>`. */
  id: string;
  from: string;
  fromPort: string;
  to: string;
  toPort: string;
  protocol: PortType;
  transport: "tcp" | "udp";
  number: number | null;
  /** The provider's service name. */
  service: string;
}

export interface Segment {
  /** A link tag's name, or `default` for unplugged NICs, or `link-<n>` for cabled groups. */
  name: string;
  modules: string[];
}

export interface NetworkPlan {
  application: string;
  services: Service[];
  intents: NetworkIntent[];
  segments: Segment[];
  warnings: string[];
}

export const serviceName = (module: string, application: string) => `${module}.${application}.internal`;

/** The environment variable a consumer reads to find `provider` (ADR 0004). */
export const serviceEnvName = (provider: string) => `APELORA_SERVICE_${provider.toUpperCase().replaceAll("-", "_")}`;

type Terminal = { module: string; port: Port };

/** Every link in the design: drawn cables and pairs of terminals sharing a link tag. */
function links(a: ApplicationAssembly): [Terminal, Terminal][] {
  const terminal = (e: Endpoint): Terminal | undefined => {
    const port = a.spec.modules.find((x) => x.id === e.moduleId)?.ports.find((p) => p.id === e.portId);
    return port ? { module: e.moduleId, port } : undefined;
  };
  const out: [Terminal, Terminal][] = [];
  for (const c of a.spec.connections) {
    const s = terminal(c.source);
    const t = terminal(c.target);
    if (s && t) out.push([s, t]);
  }
  const byTag = new Map<string, Terminal[]>();
  for (const l of a.spec.links ?? []) {
    const t = terminal(l.endpoint);
    if (t) byTag.set(l.tag, [...(byTag.get(l.tag) ?? []), t]);
  }
  for (const members of byTag.values()) {
    for (let i = 0; i < members.length; i++) for (let j = i + 1; j < members.length; j++) out.push([members[i]!, members[j]!]);
  }
  return out;
}

const compare = (x: string, y: string) => (x < y ? -1 : x > y ? 1 : 0);

export function compileNetwork(a: ApplicationAssembly, application: string, classOf?: ClassLookup): NetworkPlan {
  const warnings = new Set<string>();
  const intents = new Map<string, NetworkIntent>();
  const nicLinks: [string, string][] = [];
  const modules = new Map<string, ModuleInstance>(a.spec.modules.map((m) => [m.id, m]));
  /** Where the metrics output `t` is read: its class's metrics port, else the protocol default. */
  const metricsPort = (t: Terminal) => {
    const m = modules.get(t.module);
    const cls = m ? classOf?.(m.classRef.name, m.classRef.version) : undefined;
    const own = cls && m ? metricsEndpoint(cls, m.config) : undefined;
    if (!own) warnings.add(`${t.module}.${t.port.id} (metrics): its class declares no metrics port, so the rule uses ${DEFAULT_PORTS.metrics!.port}, the common default`);
    return own ?? DEFAULT_PORTS.metrics;
  };

  for (const [x, y] of links(a)) {
    if (x.port.type === "network" || y.port.type === "network") {
      if (x.port.type === "network" && y.port.type === "network") nicLinks.push([x.module, y.module]);
      continue;
    }
    const [out, inp] = x.port.direction === "output" ? [x, y] : [y, x];
    if (out.port.direction !== "output" || inp.port.direction !== "input" || out.module === inp.module) continue;
    // The consumer is the output terminal and the provider the input terminal, except for a
    // metrics output: it is an endpoint, and the collector at the input end reads it.
    const read = out.port.type === "metrics";
    const [from, to] = read ? [inp, out] : [out, inp];
    const known = read ? metricsPort(out) : DEFAULT_PORTS[to.port.type];
    if (!known) warnings.add(`${to.module}.${to.port.id} (${to.port.type}) has no well-known port: the rule allows any TCP port until one is set`);
    const id = `${from.module}.${from.port.id}->${to.module}.${to.port.id}`;
    intents.set(id, {
      id,
      from: from.module,
      fromPort: from.port.id,
      to: to.module,
      toPort: to.port.id,
      protocol: to.port.type,
      transport: known?.transport ?? "tcp",
      number: known?.port ?? null,
      service: serviceName(to.module, application),
    });
  }

  const sorted = [...intents.values()].sort((p, q) => compare(p.id, q.id));
  const providers = new Map<string, Map<string, ServiceEndpoint>>();
  for (const i of sorted) {
    const eps = providers.get(i.to) ?? new Map<string, ServiceEndpoint>();
    eps.set(i.toPort, { port: i.toPort, protocol: i.protocol, transport: i.transport, number: i.number });
    providers.set(i.to, eps);
  }
  const services: Service[] = [...providers.entries()]
    .sort(([p], [q]) => compare(p, q))
    .map(([module, eps]) => ({ module, name: serviceName(module, application), endpoints: [...eps.values()].sort((p, q) => compare(p.port, q.port)) }));

  // NIC segments: tagged NICs join the tag's segment, cabled NICs join each other, the rest
  // share the default segment.
  const parent = new Map<string, string>();
  const find = (n: string): string => { const p = parent.get(n) ?? n; if (p === n) return n; const r = find(p); parent.set(n, r); return r; };
  const union = (p: string, q: string) => { const [r1, r2] = [find(p), find(q)].sort(compare); if (r1 !== r2) parent.set(r2!, r1!); };
  const nicTag = new Map<string, string>();
  for (const l of a.spec.links ?? []) {
    const port = a.spec.modules.find((x) => x.id === l.endpoint.moduleId)?.ports.find((p) => p.id === l.endpoint.portId);
    if (port?.type === "network") nicTag.set(l.endpoint.moduleId, l.tag);
  }
  for (const [p, q] of nicLinks) union(`a:${p}`, `a:${q}`);
  for (const [module, tag] of nicTag) union(`t:${tag}`, `a:${module}`);
  const groups = new Map<string, string[]>();
  for (const x of a.spec.modules) {
    if (!x.ports.some((p) => p.type === "network")) continue;
    const linked = nicLinks.some(([p, q]) => p === x.id || q === x.id) || nicTag.has(x.id);
    const root = linked ? find(`a:${x.id}`) : "default";
    groups.set(root, [...(groups.get(root) ?? []), x.id]);
  }
  let n = 0;
  const segments: Segment[] = [...groups.entries()]
    .map(([root, members]) => {
      const tag = [...nicTag.entries()].find(([module]) => members.includes(module))?.[1];
      return { name: root === "default" ? "default" : (tag ?? ""), modules: [...members].sort(compare) };
    })
    .sort((p, q) => compare(p.modules[0] ?? "", q.modules[0] ?? ""))
    .map((s) => (s.name ? s : { ...s, name: `link-${++n}` }));

  return { application, services, intents: sorted, segments, warnings: [...warnings].sort(compare) };
}

export interface PublishedPort {
  module: string;
  transport: "tcp" | "udp";
  port: number;
}

/**
 * Ports reachable from outside the application: a gateway (the outside world in the design)
 * connected to a deployed module's terminal with a well-known port. Sorted, without
 * duplicates; terminals without a port number cannot be published.
 */
export function publishedPorts(plan: NetworkPlan, gateways: ReadonlySet<string>, deployed: ReadonlySet<string>): PublishedPort[] {
  const seen = new Map<string, PublishedPort>();
  for (const i of plan.intents) {
    if (!gateways.has(i.from) || !deployed.has(i.to) || i.number === null) continue;
    seen.set(`${i.to}/${i.transport}/${i.number}`, { module: i.to, transport: i.transport, port: i.number });
  }
  return [...seen.values()].sort((p, q) => compare(`${p.module}/${p.port}`, `${q.module}/${q.port}`));
}

/** Whether the plan lets `from` reach `to` on `number` (deny by default). */
export function allows(plan: NetworkPlan, from: string, to: string, number: number): boolean {
  return plan.intents.some((i) => i.from === from && i.to === to && (i.number === null || i.number === number));
}
