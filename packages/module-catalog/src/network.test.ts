import { describe, expect, it } from "vitest";
import { addModule, allows, compileNetwork, connect, createEmptyAssembly, publishedPorts, tagPort, type ApplicationAssembly } from "@apelora/assembly-model";
import { lookupClass, sampleCommerceStack, sampleNetworkedStack } from "./index";

const ids = (a: ReturnType<typeof compileNetwork>) => a.intents.map((i) => `${i.id} ${i.transport}/${i.number ?? "any"}`);

describe("network intents", () => {
  it("compile the commerce stack to one allow rule per connection, by service name", () => {
    const plan = compileNetwork(sampleCommerceStack(), "shop");
    expect(ids(plan)).toEqual([
      "gateway-01.lan->haproxy-01.frontend tcp/80",
      "grafana-01.source->postgresql-01.postgres tcp/5432",
      "haproxy-01.backend->nginx-01.http tcp/80",
      "haproxy-01.backend->nginx-02.http tcp/80",
      "nginx-01.db->postgresql-01.postgres tcp/5432",
      "nginx-02.db->postgresql-01.postgres tcp/5432",
    ]);
    expect(plan.services.map((s) => s.name)).toEqual([
      "haproxy-01.shop.internal", "nginx-01.shop.internal", "nginx-02.shop.internal", "postgresql-01.shop.internal",
    ]);
    expect(plan.intents.find((i) => i.to === "postgresql-01")?.service).toBe("postgresql-01.shop.internal");
    expect(plan.segments).toEqual([{ name: "default", modules: ["gateway-01", "grafana-01", "haproxy-01", "nginx-01", "nginx-02", "postgresql-01"] }]);
    expect(plan.warnings).toEqual([]);
  });

  it("are deterministic: the same graph in another order compiles to the same plan", () => {
    const a = sampleCommerceStack();
    const shuffled: ApplicationAssembly = { ...a, spec: { ...a.spec, modules: [...a.spec.modules].reverse(), connections: [...a.spec.connections].reverse() } };
    expect(compileNetwork(shuffled, "shop")).toEqual(compileNetwork(a, "shop"));
  });

  it("deny by default: only declared connections, in their direction and on their port", () => {
    const plan = compileNetwork(sampleCommerceStack(), "shop");
    expect(allows(plan, "nginx-01", "postgresql-01", 5432)).toBe(true);
    expect(allows(plan, "nginx-01", "postgresql-01", 22)).toBe(false);
    expect(allows(plan, "postgresql-01", "nginx-01", 80)).toBe(false);
    expect(allows(plan, "gateway-01", "postgresql-01", 5432)).toBe(false);
    expect(allows(plan, "grafana-01", "nginx-01", 80)).toBe(false);
  });

  it("link tags connect like cables; NICs on a tag form a named segment", () => {
    const plan = compileNetwork(sampleNetworkedStack(), "shop");
    expect(plan.segments).toEqual([{ name: "lan", modules: ["gateway-01", "grafana-01", "haproxy-01", "network-01", "nginx-01", "nginx-02", "postgresql-01"] }]);

    let a = createEmptyAssembly("tags");
    const place = (cls: string) => { const r = addModule(a, lookupClass(cls, "1.0.0")!, { x: 0, y: 0 }); a = r.assembly; return r.id; };
    const web = place("nginx");
    const db = place("postgresql");
    const tagged = tagPort(a, { moduleId: web, portId: "db" }, "orders-db");
    if ("error" in tagged) throw new Error(tagged.error.message);
    const both = tagPort(tagged.assembly, { moduleId: db, portId: "postgres" }, "orders-db");
    if ("error" in both) throw new Error(both.error.message);
    expect(ids(compileNetwork(both.assembly, "tags"))).toEqual([`${web}.db->${db}.postgres tcp/5432`]);
  });

  it("generic TCP terminals get a rule for any port and say so", () => {
    let a = createEmptyAssembly("raw");
    const place = (cls: string) => { const r = addModule(a, lookupClass(cls, "1.0.0")!, { x: 0, y: 0 }); a = r.assembly; return r.id; };
    const lb = place("haproxy");
    const web = place("nginx");
    const r = connect(a, { moduleId: lb, portId: "backend" }, { moduleId: web, portId: "http" });
    if ("error" in r) throw new Error(r.error.message);
    const plan = compileNetwork(r.assembly, "raw");
    expect(plan.intents[0]?.number).toBe(80);
    const generic: ApplicationAssembly = { ...r.assembly, spec: { ...r.assembly.spec, modules: r.assembly.spec.modules.map((x) => (x.id === web ? { ...x, ports: x.ports.map((p) => (p.id === "http" ? { ...p, type: "tcp" as const } : p)) } : x)) } };
    const g = compileNetwork(generic, "raw");
    expect(g.intents[0]?.number).toBeNull();
    expect(g.warnings[0]).toContain("no well-known port");
  });

  it("publishes what a gateway connects to, only when it is deployed and has a port", () => {
    const plan = compileNetwork(sampleCommerceStack(), "shop");
    const gateways = new Set(["gateway-01"]);
    expect(publishedPorts(plan, gateways, new Set(["haproxy-01", "nginx-01"]))).toEqual([{ module: "haproxy-01", transport: "tcp", port: 80 }]);
    expect(publishedPorts(plan, gateways, new Set(["nginx-01"])), "a design-only target is not published").toEqual([]);
    expect(publishedPorts(plan, new Set(), new Set(["haproxy-01"])), "no gateway, nothing published").toEqual([]);
  });

  it("a rule opens one source to one target: neither side can be swapped for another module", () => {
    const plan = compileNetwork(sampleCommerceStack(), "shop");
    // Declared: nginx-01 -> postgresql-01 on 5432, haproxy-01 -> nginx-01 on 80.
    expect(allows(plan, "nginx-01", "nginx-02", 5432), "same port, other target").toBe(false);
    expect(allows(plan, "nginx-01", "haproxy-01", 80)).toBe(false);
    expect(allows(plan, "nginx-02", "nginx-01", 80)).toBe(false);
    expect(allows(plan, "haproxy-01", "postgresql-01", 5432), "other source, declared target").toBe(false);
    expect(allows(plan, "ghost", "postgresql-01", 5432)).toBe(false);
    expect(allows(plan, "nginx-01", "ghost", 5432)).toBe(false);
    expect(allows(plan, "haproxy-01", "nginx-01", 80)).toBe(true);
    expect(allows(plan, "haproxy-01", "nginx-02", 80)).toBe(true);
  });

  it("a rule without a well-known port opens every port of that one pair, nothing else", () => {
    const plan = compileNetwork(sampleCommerceStack(), "shop");
    const anyPort = { ...plan, intents: [{ ...plan.intents[0]!, number: null }] };
    const { from, to } = anyPort.intents[0]!;
    expect(allows(anyPort, from, to, 1)).toBe(true);
    expect(allows(anyPort, from, to, 65535)).toBe(true);
    expect(allows(anyPort, to, from, 1)).toBe(false);
  });

  it("publishes each port once, sorted by module, ignoring rules that have no port", () => {
    const plan = compileNetwork(sampleCommerceStack(), "shop");
    const gw = plan.intents.find((i) => i.from === "gateway-01")!;
    const twice = { ...plan, intents: [{ ...gw, to: "zz", from: "gateway-01" }, gw, { ...gw }, { ...gw, to: "aa" }, { ...gw, to: "bb", number: null }] };
    expect(publishedPorts(twice, new Set(["gateway-01"]), new Set(["haproxy-01", "zz", "aa", "bb"]))).toEqual([
      { module: "aa", transport: "tcp", port: 80 },
      { module: "haproxy-01", transport: "tcp", port: 80 },
      { module: "zz", transport: "tcp", port: 80 },
    ]);
  });

  it("cables only count between an output and an input of two different modules", () => {
    let a = createEmptyAssembly("direction");
    const place = (cls: string) => { const r = addModule(a, lookupClass(cls, "1.0.0")!, { x: 0, y: 0 }); a = r.assembly; return r.id; };
    const web = place("nginx");
    const db = place("postgresql");
    const r = connect(a, { moduleId: web, portId: "db" }, { moduleId: db, portId: "postgres" });
    if ("error" in r) throw new Error(r.error.message);
    const plan = compileNetwork(r.assembly, "d");
    expect(plan.intents.map((i) => `${i.from}->${i.to}`)).toEqual([`${web}->${db}`]);
    // A cable written backwards still means the output reaches the input, never the reverse.
    const flipped: ApplicationAssembly = { ...r.assembly, spec: { ...r.assembly.spec, connections: r.assembly.spec.connections.map((c) => ({ ...c, source: c.target, target: c.source })) } };
    expect(compileNetwork(flipped, "d").intents.map((i) => `${i.from}->${i.to}`)).toEqual([`${web}->${db}`]);
    // Two outputs, or a cable from a module to itself, open nothing.
    const outputs: ApplicationAssembly = { ...r.assembly, spec: { ...r.assembly.spec, connections: r.assembly.spec.connections.map((c) => ({ ...c, target: { moduleId: web, portId: "db" } })) } };
    expect(compileNetwork(outputs, "d").intents).toEqual([]);
    const itself: ApplicationAssembly = { ...r.assembly, spec: { ...r.assembly.spec, connections: r.assembly.spec.connections.map((c) => ({ ...c, target: { moduleId: web, portId: "http" } })) } };
    expect(compileNetwork(itself, "d").intents).toEqual([]);
  });
});
