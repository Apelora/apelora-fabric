import { describe, expect, it } from "vitest";

import { capacityDemand, capacityFit, formatBytes, type FitServer } from "./capacity";
import type { DeployableModule } from "./runtime";

const GIB = 1024 ** 3;
const mod = (module: string, cpu: number, memoryMiB: number, replicas = 1, volumes: number[] = []): DeployableModule =>
  ({ module, name: module, replicas, resources: { cpu, memoryMiB }, volumes: volumes.map((sizeMiB, i) => ({ name: `v${i}`, sizeMiB, path: `/v${i}` })) }) as unknown as DeployableModule;
const server = (label: string, threads: number, memGiB: number, volGiB: number | null = null, accepts = true): FitServer =>
  ({ label, acceptsModules: accepts, cpuThreads: threads, memoryFreeBytes: memGiB * GIB, volumeFreeBytes: volGiB === null ? null : volGiB * GIB });

describe("capacity demand", () => {
  it("counts every replica as its own instance and sums volumes", () => {
    const d = capacityDemand([mod("web", 1, 512, 3), mod("db", 2, 2048, 1, [1024, 1024])]);
    expect(d.instances.map((i) => i.name)).toEqual(["web #1", "web #2", "web #3", "db"]);
    expect(d).toMatchObject({ cpu: 5, memoryMiB: 3584, volumeMiB: 2048 });
  });
});

describe("capacity fit", () => {
  const two = [server("a", 4, 4, 50), server("b", 4, 4, 50)];
  it("nothing to run", () => {
    expect(capacityFit(capacityDemand([]), two, { volumePool: true }).verdict).toBe("nothing-to-run");
  });
  it("fits on one server when one has room for everything", () => {
    expect(capacityFit(capacityDemand([mod("web", 1, 1024, 2)]), two, { volumePool: false }).verdict).toBe("one-server");
  });
  it("fits across servers when no single one has room", () => {
    const f = capacityFit(capacityDemand([mod("web", 1, 3072, 2)]), two, { volumePool: false });
    expect(f.verdict).toBe("across-servers");
    expect(f.notes).toEqual([]);
  });
  it("does not fit when memory runs out, saying how much", () => {
    const f = capacityFit(capacityDemand([mod("db", 2, 9 * 1024)]), two, { volumePool: false });
    expect(f.verdict).toBe("does-not-fit");
    expect(f.reasons[0]).toBe("Not enough free memory: the design asks for 9 GiB, 8 GiB is free.");
  });
  it("does not fit when the total fits but no server has room for the largest instance", () => {
    const f = capacityFit(capacityDemand([mod("db", 1, 5 * 1024)]), two, { volumePool: false });
    expect(f.verdict).toBe("does-not-fit");
    expect(f.reasons[0]).toMatch(/^No single server has room left for db/);
  });
  it("an instance cannot have more vCPUs than a server has threads; shared CPU is only a note", () => {
    expect(capacityFit(capacityDemand([mod("big", 8, 512)]), two, { volumePool: false }).reasons[0]).toBe("big asks for 8 vCPU; the largest server has 4 threads.");
    const f = capacityFit(capacityDemand([mod("web", 4, 256, 3)]), two, { volumePool: false });
    expect(f.verdict).toBe("one-server");
    expect(f.notes[0]).toBe("The design asks for 12 vCPU on 8 threads: CPU time is shared.");
  });
  it("volumes need a pool and space in it", () => {
    const d = capacityDemand([mod("db", 1, 512, 1, [60 * 1024])]);
    expect(capacityFit(d, two, { volumePool: false }).reasons[0]).toMatch(/no volume pool/);
    expect(capacityFit(d, two, { volumePool: true }).verdict).toBe("does-not-fit");
    expect(capacityFit(capacityDemand([mod("db", 1, 512, 1, [40 * 1024])]), two, { volumePool: true }).verdict).toBe("one-server");
  });
  it("a shared volume pool is one budget, not one per server", () => {
    const d = capacityDemand([mod("db", 1, 512, 1, [30 * 1024]), mod("files", 1, 512, 1, [30 * 1024])]);
    // Two local 50 GiB pools hold both across the servers; one shared 50 GiB pool (reported by every server) does not.
    expect(capacityFit(d, two, { volumePool: true }).verdict).toBe("across-servers");
    const f = capacityFit(d, two, { volumePool: true, volumePoolShared: true });
    expect(f.verdict).toBe("does-not-fit");
    expect(f.reasons[0]).toBe("Not enough volume space: the design asks for 60 GiB, 50 GiB is free.");
  });
  it("only servers that accept modules count; unreadable ones make it unknown", () => {
    expect(capacityFit(capacityDemand([mod("web", 1, 512)]), [server("a", 4, 4, null, false)], { volumePool: false }).verdict).toBe("does-not-fit");
    const unknown: FitServer = { label: "a", acceptsModules: true, cpuThreads: null, memoryFreeBytes: null, volumeFreeBytes: null };
    expect(capacityFit(capacityDemand([mod("web", 1, 512)]), [unknown], { volumePool: false }).verdict).toBe("unknown");
  });
  it("notes replicas that would have to share a server", () => {
    const f = capacityFit(capacityDemand([mod("web", 1, 3 * 1024, 3)]), [server("a", 4, 7), server("b", 4, 3)], { volumePool: false });
    expect(f.verdict).toBe("across-servers");
    expect(f.notes).toContain("Some replicas of one module would share a server.");
  });
  it("formats bytes", () => {
    expect(formatBytes(512 * 1024 * 1024)).toBe("512 MiB");
    expect(formatBytes(1.5 * GIB)).toBe("1.5 GiB");
    expect(formatBytes(380 * GIB)).toBe("380 GiB");
  });
});
