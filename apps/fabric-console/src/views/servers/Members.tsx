import { useState } from "react";

import type { ClusterGroupList, ClusterMemberDetail, ClusterMembersReport, ClusterQuorum, ClusterQuorumState, HealthServer, HealthServers, RecoveryReport, WhoAmI } from "@apelora/api-client";

import { Badge, ConfirmDialog, ErrorPanel, Section, SectionHeader, TableFrame, sentence, type Tone } from "../../components/ui";
import { client, messageOf, useLoad, type Load } from "../../lib/api";
import { holdsPlatform } from "../../lib/identity";
import { bytes } from "../Health";
import { useWide } from "../Machines";
import { EvacuateDialog, RemoveMemberDialog } from "../Membership";
import { FailedServers } from "./FailedServers";

const QUORUM: Record<ClusterQuorumState, { label: string; tone: Tone }> = {
  healthy: { label: "Majority safe", tone: "ok" },
  "at-risk": { label: "One loss from stopping", tone: "warn" },
  lost: { label: "No majority", tone: "error" },
  "not-clustered": { label: "Single server", tone: "neutral" },
  unknown: { label: "Unknown", tone: "neutral" },
};

/** The member's status in words with its colour; Incus's message on hover. */
export function MemberStatus({ m }: { m: Pick<ClusterMemberDetail, "status" | "message"> }) {
  const tone: Tone = m.status === "Online" ? "ok" : m.status === "Offline" ? "error" : "warn";
  return <Badge tone={tone} title={m.message || undefined} testId="member-status">{m.status}</Badge>;
}

const ROLE = { voter: "Voter", standby: "Stand-by", spare: "Spare" } as const;

/** Sentence-like end: the server's clauses get a full stop when they lack one. */
const stop = (s: string) => `${sentence(s)}${/[.!?]$/.test(s) ? "" : "."}`;

/** The cluster database's majority, as the server judged it. */
export function QuorumPanel({ q }: { q: ClusterQuorum }) {
  const s = QUORUM[q.state];
  const facts: [string, string][] = q.state === "not-clustered" ? [] : [
    ["Voters answering", `${q.votersUp} of ${q.voters}`],
    ["Needed", `${q.needed}`],
    ["Voters it can lose", `${q.canLose}`],
    ["Stand-bys answering", `${q.standbysUp} of ${q.standbys}`],
    ["Leader", q.leader ?? "none reported"],
  ];
  return (
    <section className="flex flex-col gap-3 rounded-box border border-base-300 p-4" aria-labelledby="quorum-title" data-testid="quorum" data-state={q.state}>
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h2 id="quorum-title" className="font-semibold">Cluster database</h2>
        <Badge tone={s.tone} testId="quorum-state">{s.label}</Badge>
      </div>
      {/* The figures say it when all is well; otherwise the server's sentence explains. */}
      {(q.state !== "healthy" || facts.length === 0) && <p className="text-sm" data-testid="quorum-summary">{q.summary}</p>}
      {facts.length > 0 && (
        <dl className="grid grid-cols-2 gap-x-6 gap-y-3 text-sm sm:grid-cols-5">
          {facts.map(([k, v]) => (
            <div key={k} className="flex flex-col gap-0.5">
              <dt className="text-base-content/70">{k}</dt>
              <dd className="font-medium tabular-nums">{v}</dd>
            </div>
          ))}
        </dl>
      )}
      {q.nextStep && <p className="text-sm text-base-content/70" data-testid="quorum-next-step">Next step: {stop(q.nextStep)}</p>}
      {q.warnings.length > 0 && (
        <ul className="flex flex-col gap-1 text-sm" aria-label="Cluster database warnings">
          {q.warnings.map((w) => <li key={w} className="flex items-start gap-2" data-testid="quorum-warning"><span className="status status-warning mt-1.5 shrink-0" aria-hidden="true" />{stop(w)}</li>)}
        </ul>
      )}
    </section>
  );
}

function Database({ m }: { m: ClusterMemberDetail }) {
  const extra = [m.eventHub && "event hub", m.ovnChassis && "OVN chassis", m.databaseExcluded && "kept out of the database"].filter(Boolean);
  return (
    <>
      <span className="flex flex-wrap items-center gap-1.5">
        {ROLE[m.databaseRole]}
        {m.leader && <span className="badge badge-sm badge-outline" data-testid="member-leader">Leader</span>}
      </span>
      {extra.length > 0 && <span className="block text-xs text-base-content/60">{extra.join(", ")}</span>}
    </>
  );
}

function Capacity({ s, loading }: { s: HealthServer | undefined; loading: boolean }) {
  const r = s?.resources;
  if (!r) return <span className="text-base-content/60">{loading ? "Reading…" : s?.resourcesError ? "Not readable" : "—"}</span>;
  const load = r.loadAverages[0];
  return (
    <span className="flex flex-col text-xs tabular-nums">
      {r.memory && <span>Memory {bytes(r.memory.usedBytes)} of {bytes(r.memory.totalBytes)}</span>}
      {r.cpuThreads != null && <span className="text-base-content/70">{load !== undefined ? `Load ${load.toFixed(2)} on ${r.cpuThreads} threads` : `${r.cpuThreads} threads`}</span>}
    </span>
  );
}

function MemberTable({ members, capacity, actions, onEvacuate, onRestore, onRemove }: {
  members: ClusterMemberDetail[]; capacity: Load<HealthServers>; actions: { evacuate: boolean; remove: boolean };
  onEvacuate: (m: string) => void; onRestore: (m: string) => void; onRemove: (m: string) => void;
}) {
  const servers = capacity.kind === "ok" ? capacity.data.items : [];
  const any = actions.evacuate || actions.remove;
  // On phones the status and database role sit under the name, so the actions stay in view.
  const wide = useWide();
  return (
    <TableFrame>
      <thead>
        <tr>
          <th>Server</th>{wide && <><th>Status</th><th>Database</th></>}
          <th className="max-sm:hidden">Failure domain</th><th className="max-md:hidden">Groups</th>
          <th className="max-sm:hidden">Incus</th><th className="max-lg:hidden">Capacity</th>
          {any && <th><span className="sr-only">Actions</span></th>}
        </tr>
      </thead>
      <tbody>
        {members.map((m) => (
          <tr key={m.name} data-testid="server-member">
            <td className="break-words">
              <span className="font-medium">{m.name}</span>
              {m.held && <span className="block text-xs text-base-content/60">Held out of placement</span>}
              {!wide && <span className="mt-1 flex flex-col items-start gap-1"><MemberStatus m={m} /><span className="text-sm"><Database m={m} /></span></span>}
            </td>
            {wide && <><td><MemberStatus m={m} /></td><td><Database m={m} /></td></>}
            <td className="max-sm:hidden">{m.failureDomain || "—"}</td>
            <td className="max-md:hidden">{m.groups.join(", ") || "—"}</td>
            <td className="max-sm:hidden" title={m.versionError ?? undefined}>
              <span className="tabular-nums">{m.incusVersion ?? "Unknown"}</span>
              <span className="block text-xs text-base-content/60">{m.architecture}</span>
            </td>
            <td className="max-lg:hidden"><Capacity s={servers.find((s) => s.name === m.name)} loading={capacity.kind === "loading"} /></td>
            {any && (
              <td className="text-right">
                {m.status === "Evacuated"
                  ? (
                    <span className="inline-flex flex-wrap justify-end gap-1">
                      {actions.evacuate && <button type="button" className="btn btn-xs btn-ghost" aria-label={`Restore ${m.name}`} onClick={() => onRestore(m.name)} data-testid="restore-member">Restore…</button>}
                      {actions.remove && <button type="button" className="btn btn-xs btn-ghost text-error" aria-label={`Remove ${m.name} from the cluster`} onClick={() => onRemove(m.name)} data-testid="remove-member">Remove…</button>}
                    </span>
                  )
                  : actions.evacuate && <button type="button" className="btn btn-xs btn-ghost" aria-label={`Evacuate ${m.name}`} disabled={m.status !== "Online"} onClick={() => onEvacuate(m.name)} data-testid="evacuate-member">Evacuate…</button>}
              </td>
            )}
          </tr>
        ))}
      </tbody>
    </TableFrame>
  );
}

function Groups({ onAuth }: { onAuth: (m: string) => void }) {
  const [groups, reload] = useLoad<ClusterGroupList>(() => client.get("cluster_groups"), onAuth);
  return (
    <Section title="Cluster groups" state={groups} testId="cluster-groups" onRetry={reload}>
      {(l) => (
        <>
          <SectionHeader title="Cluster groups" count={l.items.length} source={l.source} />
          {l.items.length === 0 ? <p className="text-sm text-base-content/70">No cluster groups.</p> : (
            <TableFrame>
              <thead><tr><th>Group</th><th className="max-sm:hidden">Description</th><th>Members</th></tr></thead>
              <tbody>
                {l.items.map((g) => (
                  <tr key={g.name} data-testid="cluster-group-row">
                    <td className="font-medium">{g.name}</td>
                    <td className="max-sm:hidden">{g.description || "—"}</td>
                    <td>{g.members.length ? g.members.join(", ") : "None"}</td>
                  </tr>
                ))}
              </tbody>
            </TableFrame>
          )}
        </>
      )}
    </Section>
  );
}

/** Servers → Members: database roles and majority, each member, its capacity, and maintenance actions. */
export function MembersTab({ me, onAuth }: { me: WhoAmI | null; onAuth: (m: string) => void }) {
  const [report, reload] = useLoad<ClusterMembersReport>(() => client.get("cluster_members"), onAuth);
  const [capacity] = useLoad<HealthServers>(() => client.get("health_servers"), onAuth);
  const [recovery, reloadRecovery] = useLoad<RecoveryReport>(() => client.get("cluster_recovery"), onAuth);
  const legacyAdmin = !!me?.permissions.includes("manage-infrastructure");
  const actions = {
    evacuate: legacyAdmin || holdsPlatform(me, "cluster.evacuate", "cluster.evacuate_unsafe"),
    remove: legacyAdmin || holdsPlatform(me, "cluster.manage"),
    confirmFenced: legacyAdmin || holdsPlatform(me, "cluster.fence_confirm"),
  };
  const [toEvacuate, setToEvacuate] = useState<string | null>(null);
  const [toRestore, setToRestore] = useState<string | null>(null);
  const [toRemove, setToRemove] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const restore = async (member: string) => {
    setError(null);
    try {
      await client.request("restore_member", { params: { member }, body: { confirm: member } });
      setNotice(`Restoring ${member}: Incus moves its instances back and starts them.`);
      reload();
    } catch (e) {
      setError(messageOf(e));
    }
  };

  return (
    <div className="flex flex-col gap-6">
      <Section title="Members" state={report} testId="server-members" onRetry={reload}>
        {(r) => (
          <div className="flex flex-col gap-6">
            <QuorumPanel q={r.quorum} />
            {r.sites && (
              <section className="flex flex-col gap-2 rounded-box border border-base-300 p-4" aria-labelledby="sites-title" data-testid="sites">
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <h3 id="sites-title" className="font-semibold">Sites</h3>
                  <Badge tone={r.sites.planned ? "warn" : "neutral"} testId="sites-state">{r.sites.planned ? "Not started" : "One site"}</Badge>
                </div>
                <p className="text-sm" data-testid="sites-summary">{r.sites.survival}</p>
                {r.sites.warning && <p className="text-sm text-base-content/70" data-testid="sites-warning">{r.sites.warning}</p>}
              </section>
            )}
            <FailedServers report={recovery} anyOffline={r.members.some((m) => m.status === "Offline")} canConfirm={actions.confirmFenced} onChanged={() => { reloadRecovery(); reload(); }} />
            <div className="flex flex-col gap-2">
              <SectionHeader title="Members" count={r.members.length} source={r.source} />
              {(actions.evacuate || actions.remove) && r.members.length > 0 && (
                <p className="text-sm text-base-content/70">Before maintenance, evacuate a member: its instances move to other members until you restore it.</p>
              )}
              {notice && <p role="status" className="flex items-start gap-2 text-sm" data-testid="servers-notice"><span className="status status-info mt-1.5 shrink-0" aria-hidden="true" />{notice}</p>}
              {error && <ErrorPanel message={error} testId="servers-error" />}
              {r.members.length === 0
                ? <p className="text-sm text-base-content/70">{r.clustered ? "The cluster reports no members." : "This Incus server is not clustered."}</p>
                : <MemberTable members={r.members} capacity={capacity} actions={actions} onEvacuate={setToEvacuate} onRestore={setToRestore} onRemove={setToRemove} />}
              {(r.powerOffs ?? []).length > 0 && (
                <div className="flex flex-col gap-2" data-testid="power-offs">
                  <h3 className="text-sm font-semibold">Power-offs</h3>
                  <p className="text-sm text-base-content/70">Recent attempts to switch a failed server off. The address of the controller is not shown.</p>
                  <ul className="flex flex-col gap-1 text-sm">
                    {(r.powerOffs ?? []).map((p) => (
                      <li key={`${p.at}-${p.member}`} data-testid="power-off">
                        <span className="font-medium">{p.member}</span>
                        <span className="text-base-content/70"> · {p.summary}</span>
                        <time className="block text-xs text-base-content/60" dateTime={p.at}>{p.at}</time>
                      </li>
                    ))}
                  </ul>
                </div>
              )}
              {(r.rebalance ?? []).length > 0 && (
                <div className="flex flex-col gap-2" data-testid="rebalance">
                  <h3 className="text-sm font-semibold">Replica spread</h3>
                  <p className="text-sm text-base-content/70">A dry run. Nothing moves unless the worker is started with <code className="font-mono text-xs">APELORA_REBALANCE=1</code>.</p>
                  <ul className="flex flex-col gap-1 text-sm">
                    {(r.rebalance ?? []).map((m) => (
                      <li key={m.instance} data-testid="rebalance-move">
                        <span className="font-mono text-xs">{m.instance}</span> from {m.from} to {m.target}
                        <span className="block text-xs text-base-content/60">{m.reason}</span>
                      </li>
                    ))}
                  </ul>
                </div>
              )}
              {capacity.kind === "error" && r.members.length > 0 && <p className="text-xs text-base-content/60 max-lg:hidden" data-testid="capacity-unavailable">Capacity not shown: {capacity.message}</p>}
            </div>
          </div>
        )}
      </Section>

      <Groups onAuth={onAuth} />

      <EvacuateDialog member={toEvacuate} onClose={() => setToEvacuate(null)} onStarted={(m) => { setToEvacuate(null); setNotice(m); reload(); }} />
      <RemoveMemberDialog member={toRemove} onClose={() => setToRemove(null)} onRemoved={(m) => { setToRemove(null); setNotice(m); reload(); }} />
      <ConfirmDialog
        open={!!toRestore}
        title={`Restore ${toRestore ?? ""}?`}
        body={<p>Incus moves the instances it evacuated back to this member and starts them. New instances may be placed on it again.</p>}
        action="Restore member"
        typeToConfirm={toRestore ?? undefined}
        onCancel={() => setToRestore(null)}
        onConfirm={() => { const m = toRestore; setToRestore(null); if (m) void restore(m); }}
      />
    </div>
  );
}
