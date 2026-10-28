// Stubbed responses for the Console's Servers area. Shapes follow the generated API types; the
// four-member layout mirrors the test cluster (three voters, one stand-by), the numbers are made up.
import type { ClusterGroupList, ClusterMemberDetail, ClusterMembersReport, ClusterQuorum, ClusterSettingsReport, ClusterVersions, RecoveryReport, SystemInfo } from "@apelora/api-client";

export const NOW = "2026-10-21T10:00:00Z";
const source = { provider: "incus" as const, observedAt: NOW };

export const member = (name: string, over: Partial<ClusterMemberDetail> = {}): ClusterMemberDetail => ({
  name, status: "Online", message: "Fully operational", roles: ["database"], databaseRole: "voter", leader: false, eventHub: false, ovnChassis: false,
  databaseExcluded: false, upForDatabase: true, failureDomain: "rack-a", groups: ["default"], architecture: "aarch64", held: false,
  incusVersion: "7.0.1", versionError: null, ...over,
});

export const healthyQuorum: ClusterQuorum = {
  state: "healthy", voters: 3, votersUp: 3, needed: 2, standbys: 1, standbysUp: 1, spares: 0, leader: "node-1", canLose: 1,
  summary: "3 of 3 voters answer; 2 needed. 1 voter can be lost at once without stopping changes.", nextStep: null, warnings: [],
};

export const atRiskQuorum: ClusterQuorum = {
  ...healthyQuorum, state: "at-risk", votersUp: 2, canLose: 0,
  summary: "2 of 3 voters answer; 2 needed. Losing one more voter stops every change to the cluster.",
  nextStep: "Incus promotes a stand-by when a voter is lost; keep maintenance off the voters until all answer",
  warnings: ["every voter is in failure domain rack-a: one fault there can take the majority"],
};

export const behindPin: ClusterVersions = {
  pinned: "7.0.2", state: "behind-pin", ready: true, blockers: [], notes: [],
  summary: "Every member runs Incus 7.0.1; Apelora is pinned to 7.0.2, so an upgrade is due.",
  groups: [{ version: "7.0.1", members: ["node-1", "node-2", "node-3", "node-4"], againstPin: "older" }],
};

export const mixedBlocked: ClusterVersions = {
  pinned: "7.0.2", state: "mixed", ready: false,
  summary: "Members run 2 different Incus releases; Apelora is pinned to 7.0.2.",
  groups: [{ version: "7.0.1", members: ["node-2", "node-3", "node-4"], againstPin: "older" }, { version: "7.0.2", members: ["node-1"], againstPin: "same" }],
  blockers: ["offline: node-3. Incus cannot upgrade a cluster with offline members, and the cluster would end up blocked"],
  notes: ["in maintenance: node-1 (evacuated or moving instances). Restore it once its upgrade is done"],
};

export const members = (over: Partial<ClusterMembersReport> = {}): ClusterMembersReport => ({
  source, clustered: true,
  sites: {
    region: "default", site: "default", sector: "default", members: 4, connectedSites: 1, planned: false,
    survival: "Connected to site default, sector default (4 members), in region default. Surviving a site means deploying the application again in another site's sector. No other site is connected, so that deployment is not planned.",
  },
  members: [
    member("node-1", { roles: ["database-leader", "database"], leader: true }),
    member("node-2"),
    member("node-3", { failureDomain: "rack-b" }),
    member("node-4", { roles: ["database-standby"], databaseRole: "standby", eventHub: true, held: true }),
  ],
  quorum: healthyQuorum, versions: behindPin, ...over,
});

export const groups = (): ClusterGroupList => ({ source, items: [
  { name: "default", description: "Default cluster group", members: ["node-1", "node-2", "node-3", "node-4"] },
  { name: "gpu", description: "", members: [] },
] });

export const settings = (): ClusterSettingsReport => ({ source, identity: "admin", items: [
  { key: "cluster.offline_threshold", label: "Offline after (seconds)", meaning: "How long a member may miss heartbeats before it counts as offline.", value: null, default: "20", usesDefault: true },
  { key: "cluster.max_voters", label: "Database voters", meaning: "How many members vote in the cluster database (an odd number, 3 or more).", value: null, default: "3", usesDefault: true },
  { key: "cluster.join_token_expiry", label: "Join tokens expire after", meaning: "Apelora recommends one hour or less.", value: "30M", default: "3H", usesDefault: false },
  { key: "images.auto_update_cached", label: "Update cached images", meaning: "Whether images cached from image servers are refreshed automatically.", value: null, default: null, usesDefault: true },
] });

export const system = (certificates: SystemInfo["certificates"]): SystemInfo => ({
  version: "0.1.0", apiVersion: "v1", incusPinned: "v7.0.2", incusSeries: "7.0", incusTestedRelease: "7.0.1",
  incus: { connected: true, apiVersion: "1.0", serverVersion: "7.0.1", clustered: true }, certificates,
});

/** Failed servers (HA slice B): `node-3` offline, waiting for fencing or fenced. */
export const recovery = (fenced: boolean): RecoveryReport => ({
  source,
  items: [{
    member: "node-3", status: "Offline", failure: "6f1c2a4e-0000-4000-8000-000000000001",
    detectedAt: "2026-10-21T09:58:10Z", lastHeartbeat: "2026-10-21T09:57:50Z",
    fencing: fenced
      ? { condition: "fenced", reason: "its storage lease expired: node-1 reports host 3 DEAD in lvm_shared" }
      : { condition: "waiting", reason: "its storage lease stopped being renewed (FAIL); sanlock declares it dead 140 s after the last renewal, once its watchdog has reset it", nextStep: "Nothing to do: Apelora restarts its modules once the lease is dead", leaseState: "fail", observedBy: "node-1", observedAt: 1792576800 },
    fencedAt: fenced ? "2026-10-21T10:00:20Z" : null,
    evidence: fenced ? { kind: "storage-lease", at: 1792576820, observedBy: "node-1", lockspace: "lvm_shared", hostId: 3, generation: 1, summary: "sanlock on node-1 reported host 3 (generation 1) of node-3 DEAD in lvm_shared: its lease was not renewed for 140 s, so it is off or its watchdog reset it" } : null,
    confirmedBy: null, decidedAt: fenced ? "2026-10-21T10:00:20Z" : null,
    items: [
      { project: "apelora", instance: "shop-db", application: "shop", module: "db", action: fenced ? "restart" : "after-fencing", target: "node-2",
        reason: "all its disks are on the shared pool" },
      { project: "apelora", instance: "shop-files", application: "shop", module: "files", action: "wait",
        reason: "volume data is on member-local pool local" },
    ],
  }],
});

export const noRecovery = (): RecoveryReport => ({ source, items: [] });
