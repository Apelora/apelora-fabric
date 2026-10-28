import { describe, expect, it } from "vitest";
import {
  addModule, allows, compileNetwork, connect, createEmptyAssembly, instanceRequestFor, metricsEndpoint, metricsProblems, MetricsSchema, readableMetrics, setConfig,
  type ApplicationAssembly, type ModuleClass,
} from "@apelora/assembly-model";
import { lookupClass, MODULE_CLASSES, TEMPLATES } from "./index";

const metricsOutputs = (c: ModuleClass) => c.ports.filter((p) => p.direction === "output" && p.type === "metrics");

/** An assembly with `cls` placed, its metrics output cabled to a Prometheus `scrape` input. */
function scraped(cls: string, config?: Record<string, string>) {
  let a = createEmptyAssembly("mon");
  const place = (c: string) => { const r = addModule(a, lookupClass(c, "1.0.0")!, { x: 0, y: 0 }); a = r.assembly; return r.id; };
  const target = place(cls);
  const prom = place("prometheus");
  if (config) for (const [k, v] of Object.entries(config)) a = setConfig(a, target, k, v);
  const out = metricsOutputs(lookupClass(cls, "1.0.0")!)[0]!;
  const r = connect(a, { moduleId: target, portId: out.id }, { moduleId: prom, portId: "scrape" });
  if ("error" in r) throw new Error(r.error.message);
  return { a: r.assembly, target, prom, port: out.id };
}

describe("metrics descriptions", () => {
  it("every catalog class describes its metrics, schema-valid and consistent", () => {
    for (const c of MODULE_CLASSES) {
      expect(c.metrics, `${c.id} has metrics`).toBeDefined();
      expect(MetricsSchema.safeParse(c.metrics).success, `${c.id} metrics schema`).toBe(true);
      expect(metricsProblems(c), c.id).toEqual([]);
    }
  });

  it("a class offers a mon output exactly when a collector can read its metrics as text on a port", () => {
    for (const c of MODULE_CLASSES) {
      const outs = metricsOutputs(c);
      expect(outs.length, `${c.id}: at most one metrics output`).toBeLessThanOrEqual(1);
      if (outs.length) expect(c.metrics?.port, `${c.id}: a mon output needs the class's metrics port`).toBeDefined();
      expect(outs.length === 1, `${c.id}: mon output ⇔ readable metrics (${c.metrics?.source}/${c.metrics?.format})`).toBe(readableMetrics(c.metrics));
    }
  });

  it("metrics outputs describe the rule, not a product", () => {
    for (const c of MODULE_CLASSES) for (const p of metricsOutputs(c)) expect(p.description, `${c.id}.${p.id}`).not.toMatch(/prometheus/i);
  });

  it("classes without a process of their own say their numbers come from Incus", () => {
    for (const id of ["gateway", "network"]) {
      const m = lookupClass(id, "1.0.0")!.metrics!;
      expect(m.source, id).toBe("platform");
      expect(m.port, id).toBeUndefined();
      expect(m.note, id).toMatch(/Incus/);
    }
  });

  it("operating-system classes rely on Incus and offer an optional agent, without a mon output", () => {
    const systems = MODULE_CLASSES.filter((x) => x.subcategory === "Linux systems" || x.subcategory === "Windows systems");
    expect(systems.map((c) => c.id)).toEqual(expect.arrayContaining(["linux-base", "debian", "opensuse", "windows-server", "windows-11", "windows-10"]));
    for (const c of systems) {
      expect(c.metrics?.source, c.id).toBe("agent");
      expect(c.metrics?.onByDefault, c.id).toBe(false);
      expect(c.metrics?.note, c.id).toMatch(/Incus/);
      expect(metricsOutputs(c), c.id).toEqual([]);
    }
  });

  it("records the findings that change how a module is wired", () => {
    const m = (id: string) => lookupClass(id, "1.0.0")!.metrics!;
    expect(m("keycloak").port).toBe(9000);
    expect(m("keycloak").note).toMatch(/management port 9000/);
    expect(m("vault").note).toMatch(/active node/);
    for (const id of ["opnsense", "pfsense"]) expect(m(id).note, id).toMatch(/only as a virtual machine/);
    for (const id of ["vault", "etcd", "otel", "logstash", "neo4j", "pocketid", "gitlab", "nextcloud"]) expect(m(id).localOnly, id).toBe(true);
    // An endpoint without authentication says it must stay on the application network.
    for (const c of MODULE_CLASSES) {
      if (readableMetrics(c.metrics) && c.metrics.auth === "none" && c.metrics.source === "native") {
        expect(`${c.metrics.note ?? ""} ${c.metrics.enable ?? ""}`, `${c.id} warns about an open endpoint`).toMatch(/application network|internal|collector's address|localhost|proxy/);
      }
    }
  });

  it("unverified facts are marked, and the port is real or chosen by the class", () => {
    const unverified = MODULE_CLASSES.filter((c) => c.metrics?.confidence === "unverified").map((c) => c.id);
    expect(unverified).toEqual(expect.arrayContaining(["neo4j", "activemq", "rd-gateway", "fedora"]));
    for (const id of ["haproxy", "envoy", "caddy", "tomcat", "openjdk", "cassandra", "kafka", "activemq"]) expect(lookupClass(id, "1.0.0")!.metrics!.portChosen, id).toBe(true);
  });
});

describe("metrics of deployable classes", () => {
  it("no runtime turns metrics on yet: instance requests carry limits, deploy-time values and the class's own settings, not a metrics listener", () => {
    const deployable = MODULE_CLASSES.filter((c) => c.runtime);
    expect(deployable.length).toBeGreaterThan(0);
    for (const c of deployable) {
      expect(c.metrics, c.id).toBeDefined();
      const r = instanceRequestFor(c, {}, { project: "p", name: "n" });
      if (!r.ok) throw new Error(r.error);
      expect(Object.keys(r.request.config ?? {}).every((k) => k.startsWith("limits.") || k.startsWith("environment.") || k === "oci.entrypoint"), c.id).toBe(true);
      // A command carries settings; none of them starts an exporter or a metrics listener.
      expect(r.request.config?.["oci.entrypoint"] ?? "", c.id).not.toMatch(/metric|export|prometheus/i);
    }
    // NGINX's status page needs a configuration file the OCI runtime cannot add yet.
    expect(lookupClass("nginx", "1.0.0")!.metrics!.note).toMatch(/does not turn it on yet/);
    // The operating-system classes rely on what Incus reports; the agent is optional.
    for (const id of ["debian", "linux-base"]) expect(lookupClass(id, "1.0.0")!.metrics!.source, id).toBe("agent");
  });
});

describe("metrics connections (deny by default)", () => {
  it("a collector's scrape input reaches the module on exactly its class's metrics port", () => {
    for (const c of MODULE_CLASSES.filter((x) => metricsOutputs(x).length)) {
      const { a, target, prom, port } = scraped(c.id);
      const plan = compileNetwork(a, "mon", lookupClass);
      const want = metricsEndpoint(c, undefined)!.port;
      expect(plan.intents.map((i) => `${i.id} ${i.transport}/${i.number}`), c.id).toEqual([`${prom}.scrape->${target}.${port} tcp/${want}`]);
      expect(allows(plan, prom, target, want), `${c.id}: collector → module`).toBe(true);
      expect(allows(plan, target, prom, want), `${c.id}: never the reverse`).toBe(false);
      if (want !== 9100) expect(allows(plan, prom, target, 9100), `${c.id}: not the generic 9100`).toBe(false);
      expect(plan.warnings, c.id).toEqual([]);
    }
  });

  it("follows the chosen version where the port depends on it", () => {
    const v3 = scraped("influxdb", { version: "3.0" });
    expect(compileNetwork(v3.a, "mon", lookupClass).intents[0]?.number).toBe(8181);
    const v27 = scraped("influxdb");
    expect(compileNetwork(v27.a, "mon", lookupClass).intents[0]?.number).toBe(8086);
    expect(compileNetwork(scraped("solr", { version: "9.6" }).a, "mon", lookupClass).intents[0]?.number).toBe(8989);
  });

  it("without the class's metrics (no lookup, or a custom class) it falls back to the protocol default and says so", () => {
    const { a } = scraped("postgresql");
    const plan = compileNetwork(a, "mon");
    expect(plan.intents[0]?.number).toBe(9100);
    expect(plan.warnings[0]).toContain("declares no metrics port");
  });

  it("templates that cable mon outputs compile to each class's own port", () => {
    let seen = 0;
    for (const t of TEMPLATES) {
      const a: ApplicationAssembly = t.build();
      const plan = compileNetwork(a, "tpl", lookupClass);
      for (const i of plan.intents.filter((x) => x.protocol === "metrics")) {
        const m = a.spec.modules.find((x) => x.id === i.to)!;
        expect(i.number, `${t.id} ${i.id}`).toBe(metricsEndpoint(lookupClass(m.classRef.name, m.classRef.version)!, m.config)!.port);
        expect(lookupClass(a.spec.modules.find((x) => x.id === i.from)!.classRef.name, "1.0.0")?.id, `${t.id}: the collector reaches the module`).toBe("prometheus");
        seen++;
      }
      expect(plan.warnings.filter((w) => w.includes("metrics")), t.id).toEqual([]);
    }
    expect(seen).toBeGreaterThan(0);
  });
});
