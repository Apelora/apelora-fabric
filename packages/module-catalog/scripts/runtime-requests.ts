// Prints the create-instance request for every catalog runtime and every image-selecting
// setting, as JSON, for the real Incus test (`catalog_runtimes_run` in
// services/deployment-worker/tests/real_incus.rs). A class whose runtime applies settings is
// also created with values other than its defaults, and `check` (a shell command run in the
// instance, exit status 0 once it is right) proves the software took them.
import { instanceRequestFor, type ModuleInstance } from "@apelora/assembly-model";
import { MODULE_CLASSES } from "../src/classes";

const CHECKS: Record<string, { config: ModuleInstance["config"]; check: string }> = {
  // 256 MiB is 268435456 bytes.
  valkey: {
    config: { maxmemory: 256, eviction: "noeviction", "append-only": true },
    check: "[ \"$(valkey-cli config get maxmemory | tail -n 1)\" = 268435456 ] && [ \"$(valkey-cli config get maxmemory-policy | tail -n 1)\" = noeviction ] && [ \"$(valkey-cli config get appendonly | tail -n 1)\" = yes ] && [ \"$(valkey-cli config get protected-mode | tail -n 1)\" = no ]",
  },
  "uptime-kuma": { config: {}, check: "curl -s -o /dev/null -f http://127.0.0.1:3001/" },
  "blackbox-exporter": { config: {}, check: "wget -qO- 'http://127.0.0.1:9115/probe?target=127.0.0.1:9115&module=tcp_connect' | grep -q '^probe_success 1'" },
};

const out = [];
for (const c of MODULE_CLASSES) {
  const rt = c.runtime;
  if (!rt) continue;
  const settings = rt.imageBy ? Object.keys(rt.imageBy.images).map((v) => ({ [rt.imageBy!.property]: v })) : [{}];
  const extra = CHECKS[c.id];
  for (const config of extra ? [{ ...settings[0], ...extra.config }, ...settings.slice(1)] : settings) {
    const r = instanceRequestFor(c, { config }, { project: "PROJECT", name: "NAME" });
    if (!r.ok) throw new Error(r.error);
    out.push({ class: c.id, config, request: r.request, ...(extra ? { check: extra.check } : {}) });
  }
}
console.log(JSON.stringify(out, null, 2));
