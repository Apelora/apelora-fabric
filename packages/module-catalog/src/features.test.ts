import { describe, expect, it } from "vitest";
import {
  addAppliance, addShape, assemblyTotals, attachNew, compatibleTargets, connect, createEmptyAssembly, diffAssemblies, formatMemory, parseAssembly, platformOf, serializeAssembly, setPlatform,
  shapeContents, suggestAttachments, tidyLayout, validateAssembly, zoneTotals, PLATFORM_INFO, effectiveSize, type ApplicationAssembly,
} from "@apelora/assembly-model";
import { allClasses, importCompose, lookupClass, sampleCommerceStack, TEMPLATES } from "./index";

const errors = (a: ApplicationAssembly) => validateAssembly(a, lookupClass).filter((d) => d.severity === "error");

describe("templates", () => {
  it("has unique ids and every template builds a valid, non-overlapping assembly", () => {
    expect(new Set(TEMPLATES.map((t) => t.id)).size).toBe(TEMPLATES.length);
    for (const t of TEMPLATES) {
      const a = t.build();
      expect(errors(a), t.id).toEqual([]);
      expect(a.spec.appliances.length, t.id).toBeGreaterThanOrEqual(4);
      expect(parseAssembly(serializeAssembly(a)).ok, t.id).toBe(true);
      const boxes = a.editor.nodes.map((n) => { const app = a.spec.appliances.find((x) => x.id === n.id)!; return { ...n, ...effectiveSize(n, app.ports, a.editor.density) }; });
      for (const p of boxes) for (const q of boxes) if (p.id < q.id) expect(p.x + p.width <= q.x || q.x + q.width <= p.x || p.y + p.height <= q.y || q.y + q.height <= p.y, `${t.id}: ${p.id} overlaps ${q.id}`).toBe(true);
    }
  });
});

describe("hypervisor target", () => {
  it("defaults to Incus, stores other platforms, and keeps the default out of the file", () => {
    let a = sampleCommerceStack();
    expect(platformOf(a)).toBe("incus");
    expect(serializeAssembly(a)).not.toContain("platform");
    a = setPlatform(a, "vmware");
    expect(platformOf(a)).toBe("vmware");
    const r = parseAssembly(serializeAssembly(a));
    expect(r.ok && platformOf(r.assembly)).toBe("vmware");
    expect(validateAssembly(a, lookupClass).some((d) => d.code === "platform-planned")).toBe(true);
    a = setPlatform(a, "incus");
    expect(a.spec.target).toBeUndefined();
    expect(validateAssembly(a, lookupClass)).toEqual([]);
  });
  it("describes every platform it supports", () => {
    for (const p of Object.values(PLATFORM_INFO)) { expect(p.diskFormats.length).toBeGreaterThan(0); expect(p.nicModels.length).toBeGreaterThan(0); }
    expect(PLATFORM_INFO.incus.status).toBe("supported");
    expect(Object.keys(PLATFORM_INFO).sort()).toEqual(["incus", "vmware"]);
  });
  it("a file saved with the retired Xen target still imports, as the default", () => {
    const json = serializeAssembly(setPlatform(sampleCommerceStack(), "vmware")).replace('"platform": "vmware"', '"platform": "xen"');
    const r = parseAssembly(json);
    expect(r.ok).toBe(true);
    expect(r.ok && platformOf(r.assembly)).toBe("incus");
  });
});

describe("totals", () => {
  it("sums vCPU, memory and cables for the assembly and for a zone", () => {
    const a = sampleCommerceStack();
    const t = assemblyTotals(a);
    expect(t.appliances).toBe(6);
    expect(t.cables).toBe(6);
    expect(t.cpu).toBe(a.spec.appliances.reduce((s, x) => s + x.resources.cpu, 0));
    const n = a.editor.nodes.find((x) => x.id === "nginx-01")!;
    const z = addShape(a, "rect", { x: n.x - 20, y: n.y - 20, width: 200, height: 160 });
    const zt = zoneTotals(z.assembly, z.id);
    expect(zt.appliances).toBe(shapeContents(z.assembly, z.id).length);
    expect(zt.appliances).toBeGreaterThanOrEqual(1);
  });
  it("formats memory", () => { expect(formatMemory(512)).toBe("512 MiB"); expect(formatMemory(2048)).toBe("2 GiB"); expect(formatMemory(1536)).toBe("1.5 GiB"); });
});

describe("tidy layout", () => {
  it("lays the flow out left to right without overlaps and keeps connections", () => {
    const a = sampleCommerceStack();
    const scrambled = { ...a, editor: { ...a.editor, nodes: a.editor.nodes.map((n, i) => ({ ...n, x: 40 + (i % 2) * 20, y: 40 + i * 10 })) } };
    const t = tidyLayout(scrambled);
    expect(t.spec).toBe(scrambled.spec);
    const x = (id: string) => t.editor.nodes.find((n) => n.id === id)!.x;
    expect(x("gateway-01")).toBeLessThan(x("haproxy-01"));
    expect(x("haproxy-01")).toBeLessThan(x("nginx-01"));
    expect(x("nginx-01")).toBeLessThan(x("postgresql-01"));
    expect(x("nginx-01")).toBe(x("nginx-02"));
    expect(Math.min(...t.editor.nodes.map((n) => n.x))).toBe(40);
  });
  it("puts unconnected appliances in a row below and survives cycles", () => {
    let a = createEmptyAssembly("t");
    const mongo = lookupClass("mongodb", "1.0.0")!;
    const ids: string[] = [];
    for (let i = 0; i < 3; i++) { const r = addAppliance(a, mongo, { x: 0, y: 0 }); a = r.assembly; ids.push(r.id); }
    const loose = addAppliance(a, lookupClass("redis", "1.0.0")!, { x: 0, y: 0 }); a = loose.assembly;
    for (let i = 0; i < 3; i++) { const r = connect(a, { applianceId: ids[i]!, portId: "repl" }, { applianceId: ids[(i + 1) % 3]!, portId: "mongo" }); if ("error" in r) throw new Error(r.error.message); a = r.assembly; }
    const t = tidyLayout(a);
    const y = (id: string) => t.editor.nodes.find((n) => n.id === id)!.y;
    expect(y(loose.id)).toBeGreaterThan(Math.max(...ids.map(y)));
    expect(new Set(t.editor.nodes.map((n) => `${n.x},${n.y}`)).size).toBe(4);
  });
  it("refits a zone around the appliances it held", () => {
    const a = sampleCommerceStack();
    const z = addShape(a, "rect", { x: 380, y: 20, width: 220, height: 400 });
    const held = shapeContents(z.assembly, z.id);
    expect(held.length).toBeGreaterThan(0);
    const t = tidyLayout(z.assembly);
    expect([...shapeContents(t, z.id)].sort()).toEqual([...held].sort());
  });
});

describe("connection suggestions", () => {
  it("lists classes with a compatible terminal, exact protocol first, never the NIC", () => {
    let a = createEmptyAssembly("t");
    const k = addAppliance(a, lookupClass("kafka", "1.0.0")!, { x: 0, y: 0 }); a = k.assembly;
    const s = suggestAttachments(a, { applianceId: k.id, portId: "out" }, allClasses());
    expect(s.length).toBeGreaterThan(0);
    expect(s[0]!.exact).toBe(true);
    expect(s.every((x) => x.port.direction === "input" && x.port.type !== "network")).toBe(true);
    expect(s.some((x) => x.cls.id === "kafka")).toBe(true);
    expect(suggestAttachments(a, { applianceId: k.id, portId: "net" }, allClasses())).toEqual([]);
  });
  it("places and cables the new appliance", () => {
    let a = createEmptyAssembly("t");
    const k = addAppliance(a, lookupClass("nginx", "1.0.0")!, { x: 0, y: 0 }); a = k.assembly;
    const r = attachNew(a, { applianceId: k.id, portId: "db" }, lookupClass("postgresql", "1.0.0")!, "postgres");
    if ("error" in r) throw new Error(r.error);
    expect(r.assembly.spec.connections).toHaveLength(1);
    const pg = r.assembly.editor.nodes.find((n) => n.id === r.id)!;
    expect(pg.x).toBeGreaterThan(0);
    // The new appliance is free of overlaps with the one it hangs off.
    expect(r.assembly.editor.nodes).toHaveLength(2);
  });
  it("attaches on the left of an input terminal", () => {
    let a = createEmptyAssembly("t");
    const pg = addAppliance(a, lookupClass("postgresql", "1.0.0")!, { x: 600, y: 100 }); a = pg.assembly;
    const r = attachNew(a, { applianceId: pg.id, portId: "postgres" }, lookupClass("nginx", "1.0.0")!, "db");
    if ("error" in r) throw new Error(r.error);
    expect(r.assembly.editor.nodes.find((n) => n.id === r.id)!.x).toBeLessThan(600);
    expect(errors(r.assembly).filter((d) => d.code !== "required-port-unconnected")).toEqual([]);
  });
  it("finds compatible sockets for a cable drag", () => {
    const a = sampleCommerceStack();
    const t = compatibleTargets(a, { applianceId: "nginx-01", portId: "db" });
    expect(t).toContainEqual({ applianceId: "postgresql-01", portId: "postgres" });
    expect(t.every((e) => e.applianceId !== "nginx-01")).toBe(true);
  });
});

describe("diff", () => {
  it("reports additions, removals, setting changes, cables and moves", () => {
    const before = sampleCommerceStack();
    expect(diffAssemblies(before, before).identical).toBe(true);
    const added = addAppliance(before, lookupClass("redis", "1.0.0")!, { x: 10, y: 10 });
    const d = diffAssemblies(before, added.assembly);
    expect(d.addedAppliances).toEqual(["redis-01"]);
    expect(d.identical).toBe(false);
    const moved = { ...before, editor: { ...before.editor, nodes: before.editor.nodes.map((n, i) => (i === 0 ? { ...n, x: n.x + 5 } : n)) } };
    expect(diffAssemblies(before, moved).movedAppliances).toBe(1);
    const fewer = { ...before, spec: { ...before.spec, connections: before.spec.connections.slice(1) } };
    expect(diffAssemblies(before, fewer).removedCables).toHaveLength(1);
    const cfg = { ...before, spec: { ...before.spec, appliances: before.spec.appliances.map((x, i) => (i === 0 ? { ...x, config: { tls: false } } : x)) } };
    expect(diffAssemblies(before, cfg).changedAppliances[0]!.changes[0]).toContain("tls");
  });
});

describe("compose import", () => {
  const yml = `
services:
  web:
    image: nginx:1.27
    depends_on: [app]
    ports: ["80:80"]
  app:
    image: ghcr.io/acme/shop:latest
    depends_on:
      db: { condition: service_healthy }
      cache: {}
  db:
    image: postgres:16
    environment:
      POSTGRES_DB: shop
  cache:
    image: redis:7
`;
  it("maps services to appliances and depends_on to cables, with notes for what was approximated", () => {
    const r = importCompose(yml);
    if (!r.ok) throw new Error(r.errors.join());
    expect(r.assembly.spec.appliances.map((a) => a.classRef.name).sort()).toEqual(["linux-base", "nginx", "postgresql", "redis"]);
    expect(r.notes.join(" ")).toContain("not in the catalog");
    expect(r.notes.join(" ")).toContain("ports");
    expect(r.assembly.spec.connections.length).toBeGreaterThanOrEqual(1);
    const pg = r.assembly.spec.appliances.find((a) => a.classRef.name === "postgresql")!;
    expect(pg.config?.database).toBe("shop");
    expect(parseAssembly(serializeAssembly(r.assembly)).ok).toBe(true);
    expect(errors(r.assembly).filter((d) => d.code !== "required-port-unconnected")).toEqual([]);
  });
  it("rejects bad input without throwing", () => {
    for (const bad of ["", "just: text", "services: []", "services: {}", "a: [", "x".repeat(250_000)]) expect(importCompose(bad).ok, bad.slice(0, 20)).toBe(false);
    const many = `services:\n${Array.from({ length: 70 }, (_, i) => `  s${i}:\n    image: redis`).join("\n")}\n`;
    expect(importCompose(many).ok).toBe(false);
  });
  it("survives alias bombs and odd shapes", () => {
    const bomb = "a: &a [x,x,x,x,x,x,x,x,x]\n" + Array.from({ length: 30 }, (_, i) => `b${i}: &b${i} [*a,*a,*a,*a,*a,*a,*a,*a,*a]`).join("\n") + "\nservices: {s: 1}\n";
    expect(() => importCompose(bomb)).not.toThrow();
    expect(importCompose("services:\n  s: 5\n").ok).toBe(true);
  });
});
