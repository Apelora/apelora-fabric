import type { DeployableModule } from "./runtime";

/**
 * Does a design fit the cluster's free capacity? Pure and advisory
 * (docs/design/health-and-capacity.md): Incus does not reserve CPU or memory for limits, so
 * "fits" means "there is room now", never a promise. Memory and volume space are hard limits;
 * one instance cannot have more vCPUs than a server has threads; more vCPUs in total than
 * threads only means CPU time is shared. Root disks are not counted (image sizes are unknown
 * before deployment).
 */

const MIB = 1024 * 1024;

export interface DemandInstance {
  module: string;
  /** e.g. `web` or `web #2` for the second replica. */
  name: string;
  cpu: number;
  memoryMiB: number;
  /** All of the instance's volumes. */
  volumeMiB: number;
}

export interface CapacityDemand {
  instances: DemandInstance[];
  cpu: number;
  memoryMiB: number;
  volumeMiB: number;
}

/** What the deployable modules ask for: each replica is its own instance. */
export function capacityDemand(deployable: readonly DeployableModule[]): CapacityDemand {
  const instances: DemandInstance[] = [];
  for (const m of deployable) {
    const volumeMiB = m.volumes.reduce((s, v) => s + v.sizeMiB, 0);
    for (let r = 1; r <= Math.max(1, m.replicas); r++) {
      instances.push({ module: m.module, name: m.replicas > 1 ? `${m.name} #${r}` : m.name, cpu: m.resources.cpu, memoryMiB: m.resources.memoryMiB, volumeMiB });
    }
  }
  return {
    instances,
    cpu: instances.reduce((s, i) => s + i.cpu, 0),
    memoryMiB: instances.reduce((s, i) => s + i.memoryMiB, 0),
    volumeMiB: instances.reduce((s, i) => s + i.volumeMiB, 0),
  };
}

/** One server as the control plane reports it (`CapacityServer`), in bytes. */
export interface FitServer {
  label: string;
  acceptsModules: boolean;
  cpuThreads: number | null;
  memoryFreeBytes: number | null;
  /** Free space in the volume pool on this server; `null` when unknown or no pool. */
  volumeFreeBytes: number | null;
}

export type FitVerdict = "nothing-to-run" | "one-server" | "across-servers" | "does-not-fit" | "unknown";

export interface CapacityFit {
  verdict: FitVerdict;
  /** Why it does not fit or is unknown. */
  reasons: string[];
  /** Worth knowing even when it fits (shared CPU, replicas sharing a server). */
  notes: string[];
}

export function formatBytes(bytes: number): string {
  const gib = bytes / (1024 * MIB);
  if (gib >= 1) return `${gib >= 100 ? Math.round(gib) : Number(gib.toFixed(1))} GiB`;
  return `${Math.round(bytes / MIB)} MiB`;
}

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/**
 * `volumePoolShared`: the volume pool is one pool every server uses, so its free space is one
 * budget (each server reports the same figure) rather than one per server.
 */
export function capacityFit(demand: CapacityDemand, servers: readonly FitServer[], opts: { volumePool: boolean; volumePoolShared?: boolean }): CapacityFit {
  if (demand.instances.length === 0) return { verdict: "nothing-to-run", reasons: [], notes: [] };
  const accepting = servers.filter((s) => s.acceptsModules);
  if (accepting.length === 0) return { verdict: "does-not-fit", reasons: ["No server accepts new modules right now."], notes: [] };
  const usable = accepting.filter((s) => s.cpuThreads !== null && s.memoryFreeBytes !== null);
  if (usable.length === 0) return { verdict: "unknown", reasons: ["The servers' resources could not be read."], notes: [] };
  const needsVolumes = demand.volumeMiB > 0;
  if (needsVolumes && !opts.volumePool) {
    return { verdict: "does-not-fit", reasons: ["The design keeps volumes, and the control plane has no volume pool configured."], notes: [] };
  }

  const reasons: string[] = [];
  const notes: string[] = [];
  const maxThreads = Math.max(...usable.map((s) => s.cpuThreads!));
  for (const i of demand.instances) {
    if (i.cpu > maxThreads) reasons.push(`${i.name} asks for ${i.cpu} vCPU; the largest server has ${maxThreads} threads.`);
  }
  const memFree = usable.reduce((s, x) => s + x.memoryFreeBytes!, 0);
  if (demand.memoryMiB * MIB > memFree) reasons.push(`Not enough free memory: the design asks for ${formatBytes(demand.memoryMiB * MIB)}, ${formatBytes(memFree)} is free.`);
  const shared = !!opts.volumePoolShared;
  const volFree = shared ? Math.max(0, ...usable.map((x) => x.volumeFreeBytes ?? 0)) : usable.reduce((s, x) => s + (x.volumeFreeBytes ?? 0), 0);
  if (needsVolumes && demand.volumeMiB * MIB > volFree) reasons.push(`Not enough volume space: the design asks for ${formatBytes(demand.volumeMiB * MIB)}, ${formatBytes(volFree)} is free.`);
  const threads = usable.reduce((s, x) => s + x.cpuThreads!, 0);
  if (demand.cpu > threads) notes.push(`The design asks for ${demand.cpu} vCPU on ${plural(threads, "thread")}: CPU time is shared.`);
  if (reasons.length > 0) return { verdict: "does-not-fit", reasons, notes };

  const maxCpu = Math.max(...demand.instances.map((i) => i.cpu));
  const fitsOne = usable.some((s) => maxCpu <= s.cpuThreads! && demand.memoryMiB * MIB <= s.memoryFreeBytes! && (!needsVolumes || demand.volumeMiB * MIB <= (shared ? volFree : (s.volumeFreeBytes ?? 0))));
  if (fitsOne) return { verdict: "one-server", reasons, notes };

  // Largest first, onto the server with the most free memory, preferring servers that hold no
  // other replica of the same module (as Apelora's placement spreads replicas).
  const room = usable.map((s) => ({ s, mem: s.memoryFreeBytes!, vol: s.volumeFreeBytes ?? 0, modules: new Set<string>() }));
  const order = [...demand.instances].sort((a, b) => b.memoryMiB - a.memoryMiB);
  let sharing = false;
  let sharedVol = volFree;
  for (const i of order) {
    const fits = room.filter((r) => i.cpu <= r.s.cpuThreads! && i.memoryMiB * MIB <= r.mem && (!needsVolumes || i.volumeMiB * MIB <= (shared ? sharedVol : r.vol)));
    if (fits.length === 0) {
      return { verdict: "does-not-fit", reasons: [`No single server has room left for ${i.name} (${formatBytes(i.memoryMiB * MIB)} memory${i.volumeMiB ? `, ${formatBytes(i.volumeMiB * MIB)} volumes` : ""}).`], notes };
    }
    const spread = fits.filter((r) => !r.modules.has(i.module));
    const pick = (spread.length ? spread : fits).reduce((a, b) => (b.mem > a.mem ? b : a));
    if (!spread.length) sharing = true;
    pick.mem -= i.memoryMiB * MIB;
    if (shared) sharedVol -= i.volumeMiB * MIB;
    else pick.vol -= i.volumeMiB * MIB;
    pick.modules.add(i.module);
  }
  if (sharing) notes.push("Some replicas of one module would share a server.");
  return { verdict: "across-servers", reasons, notes };
}
