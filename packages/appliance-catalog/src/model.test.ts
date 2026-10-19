import { describe, expect, it } from "vitest";
import { APPLIANCE_CLASSES, lookupClass, sampleCommerceStack, sampleNetworkedStack } from "./index";
import {
  addAppliance, addPort, alignNodes, ensureNics, groupAppliances, groupCandidates, ungroupAppliance, usedCustomClasses, ApplianceClassSchema, type ApplianceClass, cableToTag, explainTag, removeAppliances as removeApps, renameTag, tagNet, tagPort, tagToCable, untagPort, effectiveValue, resetConfig, setConfig, validateProperty, disconnectPort, extractSubset, findFreeSpot, insertSubset, connect, createEmptyAssembly, disconnect, effectiveSize, minApplianceHeight, movePort,
  parseAssembly, removeAppliances, removePort, resizeNode, serializeAssembly, setDensity, validateAssembly,
} from "@apelora/assembly-model";

const lookup = lookupClass;
const nginx = lookupClass("nginx", "1.0.0")!;
const pg = lookupClass("postgresql", "1.0.0")!;

describe("geometry", () => {
  it("fits two ports in the compact 96px card and grows from three rows", () => {
    expect(minApplianceHeight(1, 2)).toBe(96);
    expect(minApplianceHeight(2, 2)).toBe(96);
    expect(minApplianceHeight(3, 1)).toBe(114);
    expect(minApplianceHeight(8, 0)).toBe(204);
  });
});

describe("sample assembly", () => {
  it("has six appliances, six connections and no diagnostics", () => {
    const a = sampleCommerceStack();
    expect(a.spec.appliances).toHaveLength(6);
    expect(a.spec.connections).toHaveLength(6);
    expect(validateAssembly(a, lookup)).toEqual([]);
  });
  it("all catalog classes are valid and have unique port ids", () => {
    for (const c of APPLIANCE_CLASSES) expect(new Set(c.ports.map((p) => p.id)).size).toBe(c.ports.length);
  });
});

describe("connections", () => {
  const base = () => {
    let a = createEmptyAssembly("t");
    const w = addAppliance(a, nginx, { x: 0, y: 0 }); a = w.assembly;
    const d = addAppliance(a, pg, { x: 300, y: 0 }); a = d.assembly;
    return { a, w: w.id, d: d.id };
  };
  it("accepts output → input of the same type", () => {
    const { a, w, d } = base();
    expect("assembly" in connect(a, { applianceId: w, portId: "db" }, { applianceId: d, portId: "postgres" })).toBe(true);
  });
  it("rejects wrong direction, type mismatch, self and duplicates with a reason", () => {
    const { a, w, d } = base();
    const rev = connect(a, { applianceId: d, portId: "postgres" }, { applianceId: w, portId: "http" });
    expect("error" in rev && rev.error.code).toBe("direction");
    const mism = connect(a, { applianceId: w, portId: "db" }, { applianceId: w, portId: "http" });
    expect("error" in mism && mism.error.code).toBe("self-connection");
    const ok = connect(a, { applianceId: w, portId: "db" }, { applianceId: d, portId: "postgres" });
    if (!("assembly" in ok)) throw new Error("expected valid");
    const dup = connect(ok.assembly, { applianceId: w, portId: "db" }, { applianceId: d, portId: "postgres" });
    expect("error" in dup && ["duplicate", "source-full"]).toContain("error" in dup ? dup.error.code : "");
    const typ = connect(a, { applianceId: d, portId: "metrics" }, { applianceId: w, portId: "http" });
    expect("error" in typ && typ.error.code).toBe("type-mismatch");
  });
  it("rejects stale ports", () => {
    const { a, w, d } = base();
    const r = connect(a, { applianceId: w, portId: "nope" }, { applianceId: d, portId: "postgres" });
    expect("error" in r && r.error.code).toBe("port-missing");
  });
});

describe("ports and layout", () => {
  it("adding ports grows only the affected node, only when needed", () => {
    let a = sampleCommerceStack();
    const target = a.spec.appliances.find((x) => x.classRef.name === "grafana")!;
    const other = a.editor.nodes.find((n) => n.id !== target.id)!;
    for (let i = 0; i < 4; i++) a = addPort(a, target.id, "input").assembly;
    const n = a.editor.nodes.find((x) => x.id === target.id)!;
    const inst = a.spec.appliances.find((x) => x.id === target.id)!;
    expect(n.height).toBeGreaterThan(96);
    expect(effectiveSize(n, inst.ports, "compact").height).toBe(n.height);
    expect(a.editor.nodes.find((x) => x.id === other.id)!.height).toBe(other.height);
    expect(a.editor.nodes.find((x) => x.id === target.id)!.width).toBe(128);
  });
  it("removing a port removes only its connections; reordering keeps connections", () => {
    let a = sampleCommerceStack();
    const lb = a.spec.appliances.find((x) => x.classRef.name === "haproxy")!;
    const before = a.spec.connections.length;
    const moved = movePort(a, lb.id, "frontend", 1);
    expect(moved.spec.connections).toEqual(a.spec.connections);
    a = removePort(a, lb.id, "backend");
    expect(a.spec.connections.length).toBe(before - 2);
  });
  it("density keeps manually resized nodes", () => {
    let a = sampleCommerceStack();
    const id = a.spec.appliances[0]!.id;
    a = resizeNode(a, id, { width: 200, height: 160 });
    a = setDensity(a, "expanded");
    expect(a.editor.nodes.find((n) => n.id === id)).toMatchObject({ width: 200, height: 160, custom: true });
    expect(a.editor.nodes.find((n) => n.id !== id)).toMatchObject({ width: 178, height: 150 });
  });
  it("density scales positions so spacing is preserved", () => {
    const a = sampleCommerceStack();
    const b = setDensity(a, "expanded");
    const [n0, n1] = [a.editor.nodes[0]!, a.editor.nodes[1]!];
    const [m0, m1] = [b.editor.nodes[0]!, b.editor.nodes[1]!];
    expect(Math.abs((m1.x - m0.x) - (n1.x - n0.x) * (178 / 128))).toBeLessThanOrEqual(1);
    expect(setDensity(b, "compact").editor.nodes.every((n, i) => Math.abs(n.x - a.editor.nodes[i]!.x) <= 2)).toBe(true);
  });
  it("deleting an appliance deletes its edges and layout", () => {
    const a0 = sampleCommerceStack();
    const db = a0.spec.appliances.find((x) => x.classRef.name === "postgresql")!;
    const a = removeAppliances(a0, [db.id]);
    expect(a.spec.connections).toHaveLength(3);
    expect(a.editor.nodes.some((n) => n.id === db.id)).toBe(false);
    expect(disconnect(a0, a0.spec.connections.map((c) => c.id)).spec.connections).toEqual([]);
  });
});

describe("serialization", () => {
  it("round-trips deterministically", () => {
    const a = sampleCommerceStack();
    const text = serializeAssembly(a);
    const r = parseAssembly(text);
    expect(r.ok).toBe(true);
    if (r.ok) expect(serializeAssembly(r.assembly)).toBe(text);
  });
  it("rejects malformed, hostile and inconsistent input without throwing", () => {
    expect(parseAssembly("{not json").ok).toBe(false);
    expect(parseAssembly("{}").ok).toBe(false);
    expect(parseAssembly("x".repeat(10), 5).ok).toBe(false);
    const bad = JSON.parse(serializeAssembly(sampleCommerceStack()));
    bad.editor.nodes[0].x = "NaN";
    expect(parseAssembly(JSON.stringify(bad)).ok).toBe(false);
    const orphan = JSON.parse(serializeAssembly(sampleCommerceStack()));
    orphan.editor.nodes.push({ id: "ghost-01", x: 0, y: 0, width: 128, height: 96, custom: false });
    expect(parseAssembly(JSON.stringify(orphan)).ok).toBe(false);
  });
  it("flags a required input that is not connected", () => {
    let a = createEmptyAssembly("t");
    a = addAppliance(a, nginx, { x: 0, y: 0 }).assembly;
    expect(validateAssembly(a, lookup).some((d) => d.code === "required-port-unconnected")).toBe(true);
  });
});

describe("placement", () => {
  it("click-add avoids overlapping existing nodes", () => {
    const a = sampleCommerceStack();
    const first = a.editor.nodes[0]!;
    const spot = findFreeSpot(a, { x: first.x, y: first.y }, { width: 128, height: 96 });
    expect(a.editor.nodes.some((n) => spot.x < n.x + n.width && spot.x + 128 > n.x && spot.y < n.y + n.height && spot.y + 96 > n.y)).toBe(false);
  });
});

describe("context-menu operations", () => {
  it("duplicate keeps only cables between the copied appliances and gives fresh ids", () => {
    const a = sampleCommerceStack();
    const ids = a.spec.appliances.filter((x) => ["haproxy", "nginx"].includes(x.classRef.name)).map((x) => x.id);
    const sub = extractSubset(a, ids);
    expect(sub.connections.length).toBe(2); // haproxy → each nginx
    const r = insertSubset(a, sub, { x: 24, y: 24 });
    expect(r.ids.length).toBe(3);
    expect(new Set(r.assembly.spec.appliances.map((x) => x.id)).size).toBe(9);
    expect(r.assembly.spec.connections.length).toBe(8);
    expect(new Set(r.assembly.spec.connections.map((c) => c.id)).size).toBe(8);
    expect(validateAssembly(r.assembly, lookup).filter((d) => d.code !== "required-port-unconnected")).toEqual([]);
  });
  it("disconnecting a port removes only its cables", () => {
    const a = sampleCommerceStack();
    const pg = a.spec.appliances.find((x) => x.classRef.name === "postgresql")!;
    expect(disconnectPort(a, pg.id, "postgres").spec.connections.length).toBe(3);
  });
  it("aligns nodes", () => {
    const a = sampleCommerceStack();
    const ids = a.editor.nodes.map((n) => n.id);
    const l = alignNodes(a, ids, "left");
    expect(new Set(l.editor.nodes.map((n) => n.x)).size).toBe(1);
    const t = alignNodes(a, ids, "top");
    expect(new Set(t.editor.nodes.map((n) => n.y)).size).toBe(1);
  });
});

describe("configuration properties", () => {
  const pg = lookupClass("postgresql", "1.0.0")!;
  const prop = (id: string) => pg.properties!.find((p) => p.id === id)!;
  it("validates each type with an actionable hint", () => {
    expect(validateProperty(prop("max-connections"), 100)).toBeNull();
    expect(validateProperty(prop("max-connections"), 5)?.message).toMatch(/at least 10/);
    expect(validateProperty(prop("max-connections"), 12.5)?.message).toMatch(/whole number/);
    expect(validateProperty(prop("version"), "99")?.hint).toMatch(/15, 16, 17/);
    expect(validateProperty(prop("database"), "")?.message).toMatch(/required/);
    expect(validateProperty(prop("password-secret"), "Hunter2 password!")?.hint).toMatch(/Never paste/);
    expect(validateProperty(prop("password-secret"), "db-password")).toBeNull();
  });
  it("stores only overrides; defaults apply otherwise; reset removes them", () => {
    let a = sampleCommerceStack();
    const id = a.spec.appliances.find((x) => x.classRef.name === "postgresql")!.id;
    expect(effectiveValue(prop("version"), a.spec.appliances.find((x) => x.id === id)!.config)).toBe("17");
    a = setConfig(a, id, "version", "16");
    expect(a.spec.appliances.find((x) => x.id === id)!.config).toEqual({ version: "16" });
    a = setConfig(a, id, "version", undefined);
    expect(a.spec.appliances.find((x) => x.id === id)!.config).toBeUndefined();
    a = resetConfig(setConfig(a, id, "database", "shop"), id);
    expect(a.spec.appliances.find((x) => x.id === id)!.config).toBeUndefined();
  });
  it("assembly validation reports invalid configuration and round-trips it", () => {
    let a = sampleCommerceStack();
    const id = a.spec.appliances.find((x) => x.classRef.name === "postgresql")!.id;
    a = setConfig(a, id, "max-connections", 3);
    expect(validateAssembly(a, lookup).some((d) => d.code === "config-invalid" && d.applianceId === id)).toBe(true);
    a = setConfig(a, id, "max-connections", 250);
    expect(validateAssembly(a, lookup)).toEqual([]);
    const r = parseAssembly(serializeAssembly(a));
    expect(r.ok && r.assembly.spec.appliances.find((x) => x.id === id)!.config).toEqual({ "max-connections": 250 });
  });
});

describe("link tags", () => {
  const web = (a: ReturnType<typeof sampleCommerceStack>) => a.spec.appliances.filter((x) => x.classRef.name === "nginx").map((x) => x.id);
  const mk = () => {
    let a = createEmptyAssembly("t");
    const g = addAppliance(a, lookupClass("grafana", "1.0.0")!, { x: 0, y: 0 }); a = g.assembly;
    const p = addAppliance(a, lookupClass("postgresql", "1.0.0")!, { x: 300, y: 0 }); a = p.assembly;
    return { a, g: g.id, p: p.id };
  };
  it("joins an output and an input by name and drops the required-input error", () => {
    const { a, g, p } = mk();
    let r = tagPort(a, { applianceId: g, portId: "source" }, "db");
    if (!("assembly" in r)) throw new Error("expected ok");
    r = tagPort(r.assembly, { applianceId: p, portId: "postgres" }, "db");
    if (!("assembly" in r)) throw new Error("expected ok");
    expect(tagNet(r.assembly, "db")).toHaveLength(2);
    const diags = validateAssembly(r.assembly, lookup);
    expect(diags.some((d) => d.code === "required-port-unconnected" && d.applianceId === p)).toBe(false);
    expect(diags.some((d) => d.code.startsWith("tag-"))).toBe(false);
  });
  it("rejects bad names and incompatible protocols with a reason", () => {
    const { a, g, p } = mk();
    expect(explainTag(a, { applianceId: g, portId: "source" }, "Bad Name")?.code).toBe("tag-name");
    const r = tagPort(a, { applianceId: g, portId: "source" }, "x");
    if (!("assembly" in r)) throw new Error();
    // postgres output cannot feed an http input
    const bad = explainTag(r.assembly, { applianceId: p, portId: "metrics" }, "x");
    expect(bad).toBeNull(); // output + output is fine
    const web1 = addAppliance(r.assembly, lookupClass("nginx", "1.0.0")!, { x: 0, y: 200 });
    expect(explainTag(web1.assembly, { applianceId: web1.id, portId: "http" }, "x")?.code).toBe("tag-type");
  });
  it("warns about tags without a source or a listener", () => {
    const { a, g } = mk();
    const r = tagPort(a, { applianceId: g, portId: "source" }, "orphan");
    if (!("assembly" in r)) throw new Error();
    expect(validateAssembly(r.assembly, lookup).some((d) => d.code === "tag-no-target")).toBe(true);
  });
  it("converts a cable to a tag and back; untag and appliance removal clean up", () => {
    const a = sampleCommerceStack();
    const conn = a.spec.connections[0]!;
    const r = cableToTag(a, conn.id, "edge");
    if (!("assembly" in r)) throw new Error("expected ok");
    expect(r.assembly.spec.connections.length).toBe(5);
    expect(tagNet(r.assembly, "edge")).toHaveLength(2);
    const back = tagToCable(r.assembly, "edge");
    if (!("assembly" in back)) throw new Error("expected ok");
    expect(back.assembly.spec.connections.length).toBe(6);
    expect(back.assembly.spec.links).toBeUndefined();
    expect(untagPort(r.assembly, conn.source).spec.links).toHaveLength(1);
    expect(removeApps(r.assembly, [conn.source.applianceId]).spec.links ?? []).toHaveLength(1);
    void web; void renameTag;
  });
  it("round-trips through JSON", () => {
    const a = sampleCommerceStack();
    const r = cableToTag(a, a.spec.connections[0]!.id, "edge");
    if (!("assembly" in r)) throw new Error();
    const p = parseAssembly(serializeAssembly(r.assembly));
    expect(p.ok && p.assembly.spec.links?.length).toBe(2);
  });
});

describe("network interface (NIC)", () => {
  it("every built-in appliance has a net terminal; the Network appliance offers net outputs", () => {
    for (const c of APPLIANCE_CLASSES) {
      const net = c.ports.find((p) => p.id === "net");
      expect(net, `${c.id} has net`).toBeDefined();
      if (c.id === "network") expect(net!.direction).toBe("output");
      else expect(net).toMatchObject({ direction: "input", type: "network", optional: true, multiplicity: "one" });
    }
  });
  it("a NIC can plug into a network by cable, and a second cable is refused", () => {
    let a = createEmptyAssembly("t");
    const n = addAppliance(a, lookupClass("network", "1.0.0")!, { x: 0, y: 0 }); a = n.assembly;
    const w = addAppliance(a, lookupClass("nginx", "1.0.0")!, { x: 200, y: 0 }); a = w.assembly;
    const r = connect(a, { applianceId: n.id, portId: "net" }, { applianceId: w.id, portId: "net" });
    if (!("assembly" in r)) throw new Error("expected ok");
    const again = connect(r.assembly, { applianceId: n.id, portId: "net" }, { applianceId: w.id, portId: "net" });
    expect("error" in again).toBe(true);
    // a service port is not a NIC
    expect("error" in connect(a, { applianceId: n.id, portId: "net" }, { applianceId: w.id, portId: "http" })).toBe(true);
  });
  it("migrates older assemblies that lack the interface", () => {
    const a = sampleCommerceStack();
    const stripped = { ...a, spec: { ...a.spec, appliances: a.spec.appliances.map((x) => ({ ...x, ports: x.ports.filter((p) => p.id !== "net") })) } };
    const fixed = ensureNics(stripped, lookup);
    expect(fixed.spec.appliances.every((x) => x.ports.some((p) => p.id === "net"))).toBe(true);
    expect(ensureNics(fixed, lookup)).toBe(fixed);
  });
});

describe("components (group / ungroup)", () => {
  const stack = () => sampleCommerceStack();
  const pick = (a: ReturnType<typeof stack>, names: string[]) => a.spec.appliances.filter((x) => names.includes(x.classRef.name)).map((x) => x.id);
  const expose = (a: ReturnType<typeof stack>, ids: string[]) => groupCandidates(a, ids).filter((c) => c.external).map((c, i) => ({ inner: { applianceId: c.appliance.id, portId: c.port.id }, name: `${c.port.name}${i}` }));

  it("lists terminals that link outside the selection as candidates", () => {
    const a = stack();
    const ids = pick(a, ["haproxy", "nginx"]);
    const ext = groupCandidates(a, ids).filter((c) => c.external);
    expect(ext.map((c) => `${c.appliance.classRef.name}.${c.port.id}`).sort()).toEqual(["haproxy.frontend", "nginx.db", "nginx.db"]);
  });

  it("groups three appliances into one component, re-plugging external cables", () => {
    const a = stack();
    const ids = pick(a, ["haproxy", "nginx"]);
    const r = groupAppliances(a, ids, "Web tier", expose(a, ids), new Set());
    if (!r.ok) throw new Error(r.errors.join("; "));
    expect(r.dropped).toBe(0);
    expect(r.assembly.spec.appliances).toHaveLength(4);
    const inst = r.assembly.spec.appliances.find((x) => x.id === r.instanceId)!;
    expect(inst.ports.some((p) => p.id === "net")).toBe(true);
    expect(r.assembly.spec.connections).toHaveLength(4); // gateway→comp, comp→pg ×2, grafana→pg
    expect(ApplianceClassSchema.safeParse(r.cls).success).toBe(true);
    expect(r.cls.resources.cpu).toBe(3);
    const lk = (name: string, v: string) => (name === r.cls.id ? r.cls : lookupClass(name, v));
    expect(validateAssembly(r.assembly, lk)).toEqual([]);
    // round-trips through JSON
    const parsed = parseAssembly(serializeAssembly(r.assembly));
    expect(parsed.ok).toBe(true);
  });

  it("drops cables to terminals that are not exposed and counts them; rejects bad input", () => {
    const a = stack();
    const ids = pick(a, ["haproxy", "nginx"]);
    const only = expose(a, ids).slice(0, 1); // just the load balancer frontend
    const r = groupAppliances(a, ids, "Web tier", only, new Set());
    if (!r.ok) throw new Error("expected ok");
    expect(r.dropped).toBe(2);
    expect(groupAppliances(a, ids.slice(0, 1), "x", [], new Set()).ok).toBe(false);
    expect(groupAppliances(a, ids, "  ", only, new Set()).ok).toBe(false);
    const dup = groupAppliances(a, ids, "Dup", [{ ...only[0]!, name: "a" }, { inner: { applianceId: ids[1]!, portId: "db" }, name: "db" }, { inner: { applianceId: ids[2]!, portId: "db" }, name: "DB" }], new Set());
    expect(dup.ok).toBe(false);
  });

  it("ungroup restores the appliances, internal cables and external cables", () => {
    const a = stack();
    const ids = pick(a, ["haproxy", "nginx"]);
    const g = groupAppliances(a, ids, "Web tier", expose(a, ids), new Set());
    if (!g.ok) throw new Error();
    const u = ungroupAppliance(g.assembly, g.instanceId, g.cls);
    if ("error" in u) throw new Error(u.error);
    expect(u.assembly.spec.appliances).toHaveLength(6);
    expect(u.assembly.spec.connections).toHaveLength(6);
    expect(new Set(u.assembly.spec.connections.map((c) => c.id)).size).toBe(6);
    const lk = (n: string, v: string) => lookupClass(n, v);
    expect(validateAssembly(u.assembly, lk)).toEqual([]);
  });

  it("the component NIC maps onto every inner NIC; exports include nested custom classes", () => {
    const a = stack();
    const ids = pick(a, ["haproxy", "nginx"]);
    const g = groupAppliances(a, ids, "Web tier", expose(a, ids), new Set());
    if (!g.ok) throw new Error();
    expect(g.cls.composition!.exposes.find((e) => e.port === "net")!.inner).toHaveLength(3);
    const lk = (n: string, v: string): ApplianceClass | undefined => (n === g.cls.id ? g.cls : lookupClass(n, v));
    expect(usedCustomClasses(g.assembly, lk).map((c) => c.id)).toEqual([g.cls.id]);
  });
});

describe("components and the network", () => {
  it("a component inherits the shared network tag of its inner NICs and passes it back on ungroup", () => {
    const a = sampleNetworkedStack();
    const ids = a.spec.appliances.filter((x) => ["haproxy", "nginx"].includes(x.classRef.name)).map((x) => x.id);
    const ext = groupCandidates(a, ids).filter((c) => c.external).map((c, i) => ({ inner: { applianceId: c.appliance.id, portId: c.port.id }, name: `${c.port.name}${i}` }));
    const g = groupAppliances(a, ids, "Web tier", ext, new Set());
    if (!g.ok) throw new Error(g.errors.join("; "));
    const nicLink = (g.assembly.spec.links ?? []).find((l) => l.endpoint.applianceId === g.instanceId && l.endpoint.portId === "net");
    expect(nicLink?.tag).toBe("lan");
    const lk = (n: string, v: string) => (n === g.cls.id ? g.cls : lookupClass(n, v));
    expect(validateAssembly(g.assembly, lk)).toEqual([]);
    // retag the component's NIC, then ungroup: every inner NIC follows the component
    let re = g.assembly;
    const r = renameTag(re, "lan", "backbone");
    if (!("assembly" in r)) throw new Error("rename failed");
    re = r.assembly;
    const u = ungroupAppliance(re, g.instanceId, g.cls);
    if ("error" in u) throw new Error(u.error);
    const inner = u.assembly.spec.appliances.filter((x) => ["haproxy", "nginx"].includes(x.classRef.name)).map((x) => x.id);
    for (const id of inner) expect((u.assembly.spec.links ?? []).find((l) => l.endpoint.applianceId === id && l.endpoint.portId === "net")?.tag).toBe("backbone");
    expect(validateAssembly(u.assembly, lookup)).toEqual([]);
  });
});
