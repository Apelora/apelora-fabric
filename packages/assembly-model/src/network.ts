import type { ApplicationAssembly, Endpoint, Port, PortType } from "./schema";

/**
 * Network intents: what a design's connections allow, compiled deterministically from the
 * graph (Phase 07, see the `fabric-networking` skill). A connection is a dependency plus an
 * authorisation: the consumer (an output terminal) may reach the provider (an input terminal)
 * on that protocol, addressed by the provider's stable service name. Anything not connected
 * is denied. `network` terminals are NICs: they join L2 segments, not services.
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
  appliance: string;
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
  appliances: string[];
}

export interface NetworkPlan {
  application: string;
  services: Service[];
  intents: NetworkIntent[];
  segments: Segment[];
  warnings: string[];
}

export const serviceName = (appliance: string, application: string) => `${appliance}.${application}.internal`;

type Terminal = { appliance: string; port: Port };

/** Every link in the design: drawn cables and pairs of terminals sharing a link tag. */
function links(a: ApplicationAssembly): [Terminal, Terminal][] {
  const terminal = (e: Endpoint): Terminal | undefined => {
    const port = a.spec.appliances.find((x) => x.id === e.applianceId)?.ports.find((p) => p.id === e.portId);
    return port ? { appliance: e.applianceId, port } : undefined;
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

export function compileNetwork(a: ApplicationAssembly, application: string): NetworkPlan {
  const warnings = new Set<string>();
  const intents = new Map<string, NetworkIntent>();
  const nicLinks: [string, string][] = [];

  for (const [x, y] of links(a)) {
    if (x.port.type === "network" || y.port.type === "network") {
      if (x.port.type === "network" && y.port.type === "network") nicLinks.push([x.appliance, y.appliance]);
      continue;
    }
    // The consumer is the output terminal, the provider the input terminal.
    const [from, to] = x.port.direction === "output" ? [x, y] : [y, x];
    if (from.port.direction !== "output" || to.port.direction !== "input" || from.appliance === to.appliance) continue;
    const known = DEFAULT_PORTS[to.port.type];
    if (!known) warnings.add(`${to.appliance}.${to.port.id} (${to.port.type}) has no well-known port: the rule allows any TCP port until one is set`);
    const id = `${from.appliance}.${from.port.id}->${to.appliance}.${to.port.id}`;
    intents.set(id, {
      id,
      from: from.appliance,
      fromPort: from.port.id,
      to: to.appliance,
      toPort: to.port.id,
      protocol: to.port.type,
      transport: known?.transport ?? "tcp",
      number: known?.port ?? null,
      service: serviceName(to.appliance, application),
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
    .map(([appliance, eps]) => ({ appliance, name: serviceName(appliance, application), endpoints: [...eps.values()].sort((p, q) => compare(p.port, q.port)) }));

  // NIC segments: tagged NICs join the tag's segment, cabled NICs join each other, the rest
  // share the default segment.
  const parent = new Map<string, string>();
  const find = (n: string): string => { const p = parent.get(n) ?? n; if (p === n) return n; const r = find(p); parent.set(n, r); return r; };
  const union = (p: string, q: string) => { const [r1, r2] = [find(p), find(q)].sort(compare); if (r1 !== r2) parent.set(r2!, r1!); };
  const nicTag = new Map<string, string>();
  for (const l of a.spec.links ?? []) {
    const port = a.spec.appliances.find((x) => x.id === l.endpoint.applianceId)?.ports.find((p) => p.id === l.endpoint.portId);
    if (port?.type === "network") nicTag.set(l.endpoint.applianceId, l.tag);
  }
  for (const [p, q] of nicLinks) union(`a:${p}`, `a:${q}`);
  for (const [appliance, tag] of nicTag) union(`t:${tag}`, `a:${appliance}`);
  const groups = new Map<string, string[]>();
  for (const x of a.spec.appliances) {
    if (!x.ports.some((p) => p.type === "network")) continue;
    const linked = nicLinks.some(([p, q]) => p === x.id || q === x.id) || nicTag.has(x.id);
    const root = linked ? find(`a:${x.id}`) : "default";
    groups.set(root, [...(groups.get(root) ?? []), x.id]);
  }
  let n = 0;
  const segments: Segment[] = [...groups.entries()]
    .map(([root, members]) => {
      const tag = [...nicTag.entries()].find(([appliance]) => members.includes(appliance))?.[1];
      return { name: root === "default" ? "default" : (tag ?? ""), appliances: [...members].sort(compare) };
    })
    .sort((p, q) => compare(p.appliances[0] ?? "", q.appliances[0] ?? ""))
    .map((s) => (s.name ? s : { ...s, name: `link-${++n}` }));

  return { application, services, intents: sorted, segments, warnings: [...warnings].sort(compare) };
}

/** Whether the plan lets `from` reach `to` on `number` (deny by default). */
export function allows(plan: NetworkPlan, from: string, to: string, number: number): boolean {
  return plan.intents.some((i) => i.from === from && i.to === to && (i.number === null || i.number === number));
}
