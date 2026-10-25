import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";

// What scripts/test-cluster/ui-e2e.sh hands the test, and how it reaches inside the cluster.

export const enabled = process.env.LAB_E2E === "1";
export const studioUrl = process.env.LAB_STUDIO_URL ?? "";
export const consoleUrl = process.env.LAB_CONSOLE_URL ?? "";
const apiUrl = process.env.LAB_API_URL ?? "";
const member = process.env.LAB_MEMBER_VM ?? "apelora-incus";
export const project = process.env.LAB_INCUS_PROJECT ?? "apelora";

/** The run's administrator token, read when needed and never printed. */
export const adminToken = (): string => readFileSync(process.env.LAB_TOKEN_FILE ?? "", "utf8").trim();

export async function api<T>(path: string): Promise<T> {
  const r = await fetch(apiUrl + path, { headers: { Authorization: `Bearer ${adminToken()}` } });
  if (!r.ok) throw new Error(`GET ${path}: ${r.status} ${await r.text()}`);
  return (await r.json()) as T;
}

export type ModuleStatus = { module: string; instance: string; project: string; status: string; location: string; health?: { health: string } };
export type AppStatus = { items: ModuleStatus[]; services: { module: string; project: string; address: string }[] };

/** A shell script run as root on a cluster member, on standard input (no quoting through SSH). */
export function onMember(script: string): string {
  return execFileSync("limactl", ["shell", "--workdir", "/", member, "sudo", "bash", "-s"], { input: script, encoding: "utf8", timeout: 120_000 });
}

/** A shell script run inside an instance. */
export function inInstance(instance: string, script: string): string {
  return onMember(`incus exec ${instance} --project ${project} -- bash -s <<'APELORA_LAB'\n${script}\nAPELORA_LAB\n`);
}

/** `n` HTTP requests from an instance; the last line of each answer, or FAIL. */
export function probe(from: string, address: string, port = 80, n = 8): string[] {
  const script = `for i in $(seq 1 ${n}); do r=$(timeout 3 bash -c 'exec 3<>/dev/tcp/${address}/${port}; printf "GET / HTTP/1.0\\r\\n\\r\\n" >&3; tail -1 <&3' 2>/dev/null); echo "\${r:-FAIL}"; done`;
  return inInstance(from, script).trim().split("\n");
}

/** Whether a TCP connection from an instance to address:port is let through. */
export function canConnect(from: string, address: string, port: number): boolean {
  return inInstance(from, `timeout 3 bash -c 'exec 3<>/dev/tcp/${address}/${port}' 2>/dev/null && echo OPEN || echo blocked`).trim().endsWith("OPEN");
}

type Net = Record<string, { addresses: { family: string; scope: string; address: string }[] }>;
type Listed = { name: string; type: string; state: { network: Net | null } | null };
const listInstances = (): Listed[] => JSON.parse(onMember(`incus list --project ${project} --format json`)) as Listed[];

/** `container` or `virtual-machine`, as Incus reports it (`missing` when there is no such instance). */
export const instanceType = (name: string): string => listInstances().find((i) => i.name === name)?.type ?? "missing";

/** Each instance's global IPv4 addresses, as Incus reports them. */
export function ipv4ByInstance(): Record<string, string[]> {
  const list = listInstances();
  return Object.fromEntries(list.map((i) => [i.name, Object.entries(i.state?.network ?? {})
    .filter(([nic]) => nic !== "lo")
    .flatMap(([, n]) => n.addresses.filter((a) => a.family === "inet" && a.scope === "global").map((a) => a.address))]));
}
