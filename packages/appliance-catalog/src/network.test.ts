import { describe, expect, it } from "vitest";
import { addAppliance, allows, compileNetwork, connect, createEmptyAssembly, tagPort, type ApplicationAssembly } from "@apelora/assembly-model";
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
    expect(plan.segments).toEqual([{ name: "default", appliances: ["gateway-01", "grafana-01", "haproxy-01", "nginx-01", "nginx-02", "postgresql-01"] }]);
    expect(plan.warnings).toEqual([]);
  });

  it("are deterministic: the same graph in another order compiles to the same plan", () => {
    const a = sampleCommerceStack();
    const shuffled: ApplicationAssembly = { ...a, spec: { ...a.spec, appliances: [...a.spec.appliances].reverse(), connections: [...a.spec.connections].reverse() } };
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
    expect(plan.segments).toEqual([{ name: "lan", appliances: ["gateway-01", "grafana-01", "haproxy-01", "network-01", "nginx-01", "nginx-02", "postgresql-01"] }]);

    let a = createEmptyAssembly("tags");
    const place = (cls: string) => { const r = addAppliance(a, lookupClass(cls, "1.0.0")!, { x: 0, y: 0 }); a = r.assembly; return r.id; };
    const web = place("nginx");
    const db = place("postgresql");
    const tagged = tagPort(a, { applianceId: web, portId: "db" }, "orders-db");
    if ("error" in tagged) throw new Error(tagged.error.message);
    const both = tagPort(tagged.assembly, { applianceId: db, portId: "postgres" }, "orders-db");
    if ("error" in both) throw new Error(both.error.message);
    expect(ids(compileNetwork(both.assembly, "tags"))).toEqual([`${web}.db->${db}.postgres tcp/5432`]);
  });

  it("generic TCP terminals get a rule for any port and say so", () => {
    let a = createEmptyAssembly("raw");
    const place = (cls: string) => { const r = addAppliance(a, lookupClass(cls, "1.0.0")!, { x: 0, y: 0 }); a = r.assembly; return r.id; };
    const lb = place("haproxy");
    const web = place("nginx");
    const r = connect(a, { applianceId: lb, portId: "backend" }, { applianceId: web, portId: "http" });
    if ("error" in r) throw new Error(r.error.message);
    const plan = compileNetwork(r.assembly, "raw");
    expect(plan.intents[0]?.number).toBe(80);
    const generic: ApplicationAssembly = { ...r.assembly, spec: { ...r.assembly.spec, appliances: r.assembly.spec.appliances.map((x) => (x.id === web ? { ...x, ports: x.ports.map((p) => (p.id === "http" ? { ...p, type: "tcp" as const } : p)) } : x)) } };
    const g = compileNetwork(generic, "raw");
    expect(g.intents[0]?.number).toBeNull();
    expect(g.warnings[0]).toContain("no well-known port");
  });
});
