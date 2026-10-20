// Prints the create-instance request for every catalog runtime and every image-selecting
// setting, as JSON, for the real Incus test (`catalog_runtimes_run` in
// services/deployment-worker/tests/real_incus.rs).
import { instanceRequestFor } from "@apelora/assembly-model";
import { MODULE_CLASSES } from "../src/classes";

const out = [];
for (const c of MODULE_CLASSES) {
  const rt = c.runtime;
  if (!rt) continue;
  const settings = rt.imageBy ? Object.keys(rt.imageBy.images).map((v) => ({ [rt.imageBy!.property]: v })) : [{}];
  for (const config of settings) {
    const r = instanceRequestFor(c, { config }, { project: "PROJECT", name: "NAME" });
    if (!r.ok) throw new Error(r.error);
    out.push({ class: c.id, config, request: r.request });
  }
}
console.log(JSON.stringify(out, null, 2));
