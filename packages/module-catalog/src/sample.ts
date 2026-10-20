import { connect, createEmptyAssembly, addModule, tagPort, type ApplicationAssembly } from "@apelora/assembly-model";
import { lookupClass } from "./classes";

/** The six-module "commerce stack": gateway → load balancer → 2× NGINX → PostgreSQL, with Grafana reading PostgreSQL. */
export function sampleCommerceStack(): ApplicationAssembly {
  let a = createEmptyAssembly("commerce-stack");
  a = { ...a, metadata: { name: "commerce-stack", uid: "assembly-sample-commerce" } };
  const place = (cls: string, x: number, y: number): string => {
    const r = addModule(a, lookupClass(cls, "1.0.0")!, { x, y });
    a = r.assembly;
    return r.id;
  };
  const gw = place("gateway", 40, 150);
  const lb = place("haproxy", 230, 150);
  const web1 = place("nginx", 430, 60);
  const web2 = place("nginx", 430, 250);
  const db = place("postgresql", 640, 150);
  const gf = place("grafana", 430, 400);
  const link = (s: string, sp: string, t: string, tp: string) => {
    const r = connect(a, { moduleId: s, portId: sp }, { moduleId: t, portId: tp });
    if ("error" in r) throw new Error(`sample is invalid: ${r.error.message}`);
    a = r.assembly;
  };
  link(gw, "lan", lb, "frontend");
  link(lb, "backend", web1, "http");
  link(lb, "backend", web2, "http");
  link(web1, "db", db, "postgres");
  link(web2, "db", db, "postgres");
  link(gf, "source", db, "postgres");
  return a;
}

/** The commerce stack plus a Network module: every NIC joins it through the link tag "lan" (no cables needed). */
export function sampleNetworkedStack(): ApplicationAssembly {
  let a = sampleCommerceStack();
  a = { ...a, metadata: { name: "commerce-stack-networked", uid: "assembly-sample-networked" } };
  const net = addModule(a, lookupClass("network", "1.0.0")!, { x: 40, y: 340 });
  a = net.assembly;
  const joined = (e: { moduleId: string; portId: string }) => {
    const r = tagPort(a, e, "lan");
    if ("error" in r) throw new Error(`sample is invalid: ${r.error.message}`);
    a = r.assembly;
  };
  joined({ moduleId: net.id, portId: "net" });
  for (const x of a.spec.modules) if (x.id !== net.id) joined({ moduleId: x.id, portId: "net" });
  return a;
}
