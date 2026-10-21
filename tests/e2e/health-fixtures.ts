// Stubbed responses for the Health page and the Studio's capacity chip. Shapes follow the
// generated API types; every number here is made up for tests and never shown as real.
import type { CapacityReport, HealthReport, HealthServer, HealthServers } from "@apelora/api-client";

export const NOW = "2026-10-21T10:00:00Z";
const GIB = 1024 ** 3;

export const healthy = (provider: "incus" | "mock" = "incus"): HealthReport => ({
  checkedAt: NOW,
  overall: "healthy",
  controlPlane: { state: "healthy", version: "0.1.0", apiVersion: "v1", startedAt: "2026-10-20T08:00:00Z", uptimeSecs: 93_600, auth: "tokens", provider, reason: null, nextStep: null },
  database: { state: "healthy", roundTripMs: 0.8, schemaVersion: 16, expectedSchemaVersion: 16, reason: null, nextStep: null },
  workers: { state: "healthy", items: [{ id: "worker-1", host: "cp-1", version: "0.1.0", startedAt: "2026-10-20T08:00:00Z", lastSeen: NOW, secondsSinceSeen: 4, state: "running" }], reason: null, nextStep: null },
  jobQueue: { state: "healthy", counts: { waiting: 0, retrying: 0, running: 1, stalled: 0, failedLastHour: 0, oldestWaitingSecs: null }, reason: null, nextStep: null },
  incus: provider === "mock"
    ? { state: "not-set-up", provider, apiVersion: "1.0", serverVersion: null, clustered: true, roundTripMs: 0.1, members: { total: 3, online: 3, evacuated: 0, offline: 0, other: 0 }, reason: "Mock data: no Incus server is configured", nextStep: "Set APELORA_INCUS_URL and the certificate paths to read a real cluster" }
    : { state: "healthy", provider, apiVersion: "1.0", serverVersion: "7.0.1", clustered: true, roundTripMs: 6.2, members: { total: 3, online: 3, evacuated: 0, offline: 0, other: 0 }, reason: null, nextStep: null },
  networks: { state: "healthy", total: 2, problems: [], reason: null, nextStep: null },
  storagePools: { state: "healthy", total: 2, problems: [], reason: null, nextStep: null },
  identityProviders: { state: "healthy", items: [{ name: "Example Corp", source: "console", enabled: true, reachable: true, error: null }], apiTokens: true, reason: null, nextStep: null },
});

/** One worker lost, jobs waiting, a server offline. */
export const degraded = (): HealthReport => {
  const r = healthy();
  return {
    ...r,
    overall: "down",
    workers: { state: "down", items: [{ id: "worker-1", host: "cp-1", version: "0.1.0", startedAt: "2026-10-20T08:00:00Z", lastSeen: "2026-10-21T09:58:00Z", secondsSinceSeen: 120, state: "lost" }], reason: "No deployment worker is running: deployments, moves and recovery wait until one starts", nextStep: "Start the deployment worker (apelora-deployment-worker) with the same DATABASE_URL, and read its log" },
    jobQueue: { state: "down", counts: { waiting: 3, retrying: 1, running: 0, stalled: 1, failedLastHour: 2, oldestWaitingSecs: 300 }, reason: "3 jobs wait and no worker is running", nextStep: null },
    incus: { ...r.incus, state: "degraded", members: { total: 3, online: 2, evacuated: 0, offline: 1, other: 0 }, reason: "1 of 3 servers not online: node-3 is Offline", nextStep: "See the servers below; Servers → Members has evacuate and restore" },
    identityProviders: { state: "degraded", items: [{ name: "Example Corp", source: "console", enabled: true, reachable: false, error: "connection refused" }], apiTokens: true, reason: "Not reachable: Example Corp. People who sign in with it cannot sign in until it answers (retried every minute)", nextStep: "Open Identity & Access → Sign-in and check the provider" },
  };
};

export const server = (name: string, over: Partial<HealthServer> = {}): HealthServer => ({
  name, status: "Online", message: "Fully operational", state: "healthy", roles: ["database"], architecture: "aarch64", failureDomain: "default", held: false,
  resources: { cpuThreads: 4, loadAverages: [0.42, 0.3, 0.2], memory: { usedBytes: 3 * GIB, totalBytes: 8 * GIB }, pools: [{ name: "local", usedBytes: 6 * GIB, totalBytes: 20 * GIB }], uptimeSecs: 266_400 },
  resourcesError: null,
  instances: [
    { name: "shop-web-1", project: "apelora", kind: "container", status: "Running", managed: true },
    { name: "shop-db-1", project: "apelora", kind: "virtual-machine", status: "Running", managed: true },
  ],
  warnings: [],
  ...over,
});

export const servers = (provider: "incus" | "mock" = "incus", items?: HealthServer[]): HealthServers => ({
  source: { provider, observedAt: NOW },
  clustered: true,
  items: items ?? [server("node-1", { roles: ["database-leader", "database"] }), server("node-2"), server("node-3", { instances: [] })],
});

export const troubledServers = (): HealthServers => servers("incus", [
  server("node-1", { state: "degraded", resources: { cpuThreads: 4, loadAverages: [5.1, 4.2, 3.9], memory: { usedBytes: 7.6 * GIB, totalBytes: 8 * GIB }, pools: [{ name: "local", usedBytes: 18 * GIB, totalBytes: 20 * GIB }], uptimeSecs: 266_400 },
    warnings: ["Memory 95 % used (7.6 GiB of 8.0 GiB)", "Storage pool local 90 % full (2.0 GiB free)", "CPU busy: load 5.1 on 4 threads"] }),
  server("node-2", { held: true }),
  server("node-3", { status: "Offline", message: "No heartbeat for 40s", state: "down", resources: null, resourcesError: "GET /1.0/resources?target=node-3: cannot reach the member", instances: [], warnings: ["Offline: the cluster cannot reach it; its instances are not running", "Resources unavailable: GET /1.0/resources?target=node-3: cannot reach the member"] }),
]);

export const capacity = (over: Partial<CapacityReport> = {}): CapacityReport => ({
  source: { provider: "incus", observedAt: NOW },
  named: false,
  volumePool: "local",
  volumePoolShared: false,
  servers: [
    { label: "Server 1", acceptsModules: true, reason: null, architecture: "aarch64", cpuThreads: 4, load: 0.4, memory: { usedBytes: 2 * GIB, totalBytes: 8 * GIB }, volumePool: { usedBytes: 4 * GIB, totalBytes: 20 * GIB } },
    { label: "Server 2", acceptsModules: true, reason: null, architecture: "aarch64", cpuThreads: 4, load: 0.1, memory: { usedBytes: 1 * GIB, totalBytes: 8 * GIB }, volumePool: { usedBytes: 2 * GIB, totalBytes: 20 * GIB } },
    { label: "Server 3", acceptsModules: false, reason: "It is Offline", architecture: "aarch64", cpuThreads: null, load: null, memory: null, volumePool: null },
  ],
  totals: { serversAccepting: 2, cpuThreads: 8, memoryFreeBytes: 13 * GIB, memoryTotalBytes: 16 * GIB, volumePoolFreeBytes: 34 * GIB },
  ...over,
});
