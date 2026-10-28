import { Fragment, useEffect, useRef, useState } from "react";

import type { ModuleStatus, ApplicationList, ApplicationStatus, ApplicationVolumes, SnapshotList, VolumeItem, WhoAmI } from "@apelora/api-client";

import { ApplicationActions, useApplicationOperations } from "../components/applicationOperations";
import { TerminalDialog, type TerminalTarget } from "../components/TerminalDialog";
import { ErrorPanel, FormDialog, Section, SectionHeader, TableFrame, sentence } from "../components/ui";
import { client, messageOf, useLoad } from "../lib/api";
import { holds } from "../lib/identity";
import { instanceHref } from "../lib/routes";

type Statuses = Record<string, ApplicationStatus | { error: string }>;

/** How an application's instances are doing, in a few words and a status colour. */
export function summary(items: readonly ModuleStatus[]): { label: string; dot: string } {
  if (!items.length) return { label: "No instances", dot: "status-neutral" };
  const running = items.filter((i) => i.status === "Running");
  const failing = running.filter((i) => i.health?.health === "unhealthy").length;
  const stopped = items.length - running.length;
  if (!failing && !stopped) return { label: "All running", dot: "status-success" };
  const parts = [
    failing ? `${failing} failing ${failing === 1 ? "its" : "their"} check` : "",
    stopped ? `${stopped} not running` : "",
  ].filter(Boolean);
  return { label: sentence(parts.join(", ")), dot: failing === items.length || stopped === items.length ? "status-error" : "status-warning" };
}

/** An instance's health, in words. */
/** `checked`: the module has a service address, so the worker checks it (the first decision takes a moment). */
const health = (i: ModuleStatus, checked: boolean): string => {
  if (i.status !== "Running") return "—";
  if (!i.health) return checked ? "Not checked yet" : "Not checked";
  const since = new Date(i.health.since).toLocaleTimeString();
  if (i.health.health === "unhealthy") return `Failing since ${since}${i.health.restartedAt ? `; restarted ${new Date(i.health.restartedAt).toLocaleTimeString()}` : ""}`;
  return i.health.health === "healthy" ? "Passing" : "Not decided yet";
};

const dot = (i: ModuleStatus) => (i.status !== "Running" ? "status-error" : i.health?.health === "unhealthy" ? "status-warning" : "status-success");

/** A size in MiB, in the largest whole unit. */
export function size(mib: number): string {
  if (mib >= 1024 * 1024 && mib % (1024 * 1024) === 0) return `${mib / (1024 * 1024)} TiB`;
  if (mib >= 1024 && mib % 1024 === 0) return `${mib / 1024} GiB`;
  return `${mib} MiB`;
}

const volumeState = (v: VolumeItem): { label: string; dot: string } =>
  v.state === "attached" ? { label: `In use by ${v.usedBy.join(", ")}`, dot: "status-success" }
    : v.state === "retained" ? { label: "Kept, not in use", dot: "status-neutral" }
    : { label: "Not found in Incus", dot: "status-error" };

/** Deleting a kept volume: its data is gone for good, so its name is typed to confirm. */
function DeleteVolumeDialog({ application, volume, onClose, onDeleted }: { application: string; volume: VolumeItem | null; onClose: () => void; onDeleted: () => void }) {
  const [typed, setTyped] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  useEffect(() => { setTyped(""); setError(null); }, [volume]);
  // Keep showing the volume while the dialog closes, so its text does not empty mid-animation.
  const last = useRef<VolumeItem | null>(null);
  if (volume) last.current = volume;
  const shown = volume ?? last.current;
  const name = shown?.incusName ?? "";
  const remove = async () => {
    setBusy(true);
    setError(null);
    try {
      await client.request("delete_application_volume", { params: { name: application, volume: name }, query: { confirm: typed } });
      onDeleted();
    } catch (e) {
      setError(messageOf(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <FormDialog open={!!volume} title={`Delete volume ${name}?`} onClose={onClose} testId="delete-volume-dialog">
      <p className="text-sm text-base-content/80">
        Its {shown ? size(shown.sizeMib) : ""} of data and its snapshots are deleted from pool {shown?.pool}. This cannot be undone.
      </p>
      <label className="flex flex-col gap-1 text-sm">
        <span>Type <strong className="font-mono">{name}</strong> to confirm</span>
        <input className="input input-sm w-full font-mono" autoComplete="off" spellCheck={false} value={typed} onChange={(e) => setTyped(e.target.value)} data-testid="delete-volume-confirm" />
      </label>
      {error && <ErrorPanel message={error} testId="delete-volume-error" />}
      <div className="modal-action mt-0">
        <button type="button" className="btn btn-sm btn-outline" onClick={onClose}>Cancel</button>
        <button type="button" className="btn btn-sm btn-ghost text-error" disabled={typed !== name || busy} onClick={() => void remove()} data-testid="delete-volume-start">
          {busy ? "Deleting…" : "Delete volume"}
        </button>
      </div>
    </FormDialog>
  );
}

/**
 * A volume's snapshots: take one (crash-consistent while its instance runs), restore one with
 * its name typed once the instance is stopped, delete one.
 */
function SnapshotsDialog({ application, volume, onClose }: { application: string; volume: VolumeItem | null; onClose: () => void }) {
  const [list, setList] = useState<SnapshotList | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [restoring, setRestoring] = useState<string | null>(null);
  const [typed, setTyped] = useState("");
  const [busy, setBusy] = useState(false);
  const [round, setRound] = useState(0);
  const last = useRef<VolumeItem | null>(null);
  if (volume) last.current = volume;
  const shown = volume ?? last.current;
  const params = { name: application, volume: shown?.incusName ?? "" };
  useEffect(() => { setNotice(null); setRestoring(null); setTyped(""); }, [volume]);
  useEffect(() => {
    if (!volume) return;
    let current = true;
    setError(null);
    client.request("volume_snapshots", { params: { name: application, volume: volume.incusName } }).then(
      (l) => { if (current) setList(l); },
      (e: unknown) => { if (current) setError(messageOf(e)); },
    );
    return () => { current = false; };
  }, [application, volume, round]);
  const act = async (fn: () => Promise<string>) => {
    setBusy(true);
    setError(null);
    try { setNotice(await fn()); setRestoring(null); setTyped(""); setRound((r) => r + 1); } catch (e) { setError(messageOf(e)); } finally { setBusy(false); }
  };
  const take = () => act(async () => {
    const t = await client.request("take_volume_snapshot", { params });
    return t.consistency === "crash-consistent"
      ? `Snapshot ${t.name} taken while the instance runs: like after a power cut, so a database replays its log on restore.`
      : `Snapshot ${t.name} taken.`;
  });
  const restore = (snapshot: string) => act(async () => {
    await client.request("restore_volume_snapshot", { params: { ...params, snapshot }, body: { confirm: typed } });
    return `Volume ${shown?.incusName ?? ""} is back to ${snapshot}. Start the module again.`;
  });
  const remove = (snapshot: string) => act(async () => {
    await client.request("delete_volume_snapshot", { params: { ...params, snapshot } });
    return `Snapshot ${snapshot} deleted.`;
  });
  return (
    <FormDialog open={!!volume} title={`Snapshots of ${shown?.incusName ?? ""}`} onClose={onClose} testId="snapshots-dialog" wide>
      <p className="text-sm text-base-content/80">Snapshots stay in the same pool as the volume: they protect against mistakes, not against losing the server.</p>
      {notice && <p className="text-sm" role="status" data-testid="snapshots-notice">{notice}</p>}
      {!list && !error && <p className="text-sm text-base-content/60" role="status">Reading snapshots…</p>}
      {list && (list.items.length === 0 ? <p className="text-sm text-base-content/70">No snapshots yet.</p> : (
        <TableFrame>
          <thead><tr><th>Snapshot</th><th className="max-sm:hidden">Taken</th><th><span className="sr-only">Actions</span></th></tr></thead>
          <tbody>
            {list.items.map((sn) => (
              <tr key={sn.name} data-testid="snapshot-row">
                <td className="font-mono text-sm">{sn.name}</td>
                <td className="max-sm:hidden text-sm">{Number.isNaN(Date.parse(sn.createdAt)) ? sn.createdAt : new Date(sn.createdAt).toLocaleString()}</td>
                <td className="text-right">
                  {restoring === sn.name ? (
                    <span className="inline-flex flex-wrap items-center justify-end gap-1">
                      <input className="input input-xs w-44 font-mono" aria-label={`Type ${sn.name} to restore it`} placeholder={sn.name} autoComplete="off" spellCheck={false} value={typed} onChange={(e) => setTyped(e.target.value)} data-testid="restore-confirm" />
                      <button type="button" className="btn btn-xs btn-ghost text-error" disabled={typed !== sn.name || busy} onClick={() => void restore(sn.name)} data-testid="restore-start">Restore</button>
                      <button type="button" className="btn btn-xs btn-ghost" onClick={() => { setRestoring(null); setTyped(""); }}>Cancel</button>
                    </span>
                  ) : (
                    <span className="inline-flex justify-end gap-1">
                      <button type="button" className="btn btn-xs btn-ghost" aria-label={`Restore ${sn.name}`} onClick={() => { setRestoring(sn.name); setTyped(""); }} data-testid="restore-snapshot">Restore…</button>
                      <button type="button" className="btn btn-xs btn-ghost text-error" aria-label={`Delete ${sn.name}`} disabled={busy} onClick={() => void remove(sn.name)} data-testid="delete-snapshot">Delete</button>
                    </span>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </TableFrame>
      ))}
      {restoring && <p className="text-sm text-base-content/70">Restoring replaces everything written since {restoring}. Stop the module first; type the snapshot's name to confirm.</p>}
      {error && <ErrorPanel message={error} testId="snapshots-error" />}
      <div className="modal-action mt-0">
        <button type="button" className="btn btn-sm btn-outline" onClick={onClose}>Close</button>
        <button type="button" className="btn btn-sm btn-primary" disabled={busy} onClick={() => void take()} data-testid="take-snapshot">{busy ? "Working…" : "Take snapshot"}</button>
      </div>
    </FormDialog>
  );
}

/** Growing a volume, also while it is in use. Volumes only grow, so the new size starts at the current one. */
function GrowVolumeDialog({ application, volume, onClose, onGrown }: { application: string; volume: VolumeItem | null; onClose: () => void; onGrown: (message: string) => void }) {
  const [gib, setGib] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  useEffect(() => { setGib(volume ? String(Math.ceil(volume.sizeMib / 1024) + 1) : ""); setError(null); }, [volume]);
  const last = useRef<VolumeItem | null>(null);
  if (volume) last.current = volume;
  const shown = volume ?? last.current;
  const mib = Math.round(Number(gib) * 1024);
  const valid = !!shown && Number.isFinite(mib) && mib > shown.sizeMib;
  const grow = async () => {
    if (!shown) return;
    setBusy(true);
    setError(null);
    try {
      await client.request("grow_application_volume", { params: { name: application, volume: shown.incusName }, body: { sizeMib: mib } });
      onGrown(`Volume ${shown.incusName} is now ${size(mib)}.`);
    } catch (e) {
      setError(messageOf(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <FormDialog open={!!volume} title={`Grow volume ${shown?.incusName ?? ""}`} onClose={onClose} testId="grow-volume-dialog">
      <p className="text-sm text-base-content/80">It is {shown ? size(shown.sizeMib) : ""} now. Volumes only grow; this can be done while it is in use.</p>
      <label className="flex flex-col gap-1 text-sm">
        <span>New size in GiB</span>
        <input type="number" min={1} step={1} className="input input-sm w-40 tabular-nums" value={gib} onChange={(e) => setGib(e.target.value)} data-testid="grow-volume-size" />
      </label>
      {error && <ErrorPanel message={error} testId="grow-volume-error" />}
      <div className="modal-action mt-0">
        <button type="button" className="btn btn-sm btn-outline" onClick={onClose}>Cancel</button>
        <button type="button" className="btn btn-sm btn-primary" disabled={!valid || busy} onClick={() => void grow()} data-testid="grow-volume-start">
          {busy ? "Growing…" : "Grow volume"}
        </button>
      </div>
    </FormDialog>
  );
}

/**
 * The application's persistent volumes. Removing a module keeps its volume; this is where
 * a kept volume is deleted, on purpose.
 */
function Volumes({ application }: { application: string }) {
  const [state, setState] = useState<{ kind: "loading" } | { kind: "ok"; data: ApplicationVolumes } | { kind: "error"; message: string }>({ kind: "loading" });
  const [deleting, setDeleting] = useState<VolumeItem | null>(null);
  const [growing, setGrowing] = useState<VolumeItem | null>(null);
  const [snapshotting, setSnapshotting] = useState<VolumeItem | null>(null);
  const [done, setDone] = useState<string | null>(null);
  const [round, setRound] = useState(0);
  useEffect(() => {
    let current = true;
    client.request("application_volumes", { params: { name: application } }).then(
      (data) => { if (current) setState({ kind: "ok", data }); },
      (e: unknown) => { if (current) setState({ kind: "error", message: messageOf(e) }); },
    );
    return () => { current = false; };
  }, [application, round]);
  return (
    <div className="flex flex-col gap-2" data-testid="application-volumes">
      <h3 className="text-sm font-semibold">Volumes</h3>
      {done && <p className="text-sm" role="status">{done}</p>}
      {state.kind === "loading" && <p className="text-sm text-base-content/60" role="status">Reading volumes…</p>}
      {state.kind === "error" && <ErrorPanel message={`Volumes could not be read: ${state.message}`} onRetry={() => setRound((r) => r + 1)} />}
      {state.kind === "ok" && (state.data.items.length === 0 ? <p className="text-sm text-base-content/70">No volumes.</p> : (
        <TableFrame>
          <thead><tr><th>Volume</th><th className="max-sm:hidden">Size</th><th>State</th><th><span className="sr-only">Actions</span></th></tr></thead>
          <tbody>
            {state.data.items.map((v) => {
              const st = volumeState(v);
              return (
                <tr key={v.incusName} data-testid="application-volume">
                  <td className="font-medium break-words">{v.incusName}<span className="block text-xs font-normal text-base-content/60">{v.name} of {v.module}, on {v.member || "the server"}</span></td>
                  <td className="max-sm:hidden">{size(v.sizeMib)}</td>
                  <td className="text-sm"><span className={`status mr-2 ${st.dot}`} aria-hidden="true" />{st.label}<span className="block text-xs text-base-content/60 sm:hidden">{size(v.sizeMib)}</span></td>
                  <td className="text-right">
                    <div className="flex flex-wrap justify-end gap-1">
                      {v.state !== "missing" && <button type="button" className="btn btn-xs btn-ghost" aria-label={`Snapshots of ${v.incusName}`} onClick={() => { setDone(null); setSnapshotting(v); }} data-testid="volume-snapshots">Snapshots…</button>}
                      {v.state !== "missing" && <button type="button" className="btn btn-xs btn-ghost" aria-label={`Grow ${v.incusName}`} onClick={() => { setDone(null); setGrowing(v); }} data-testid="grow-volume">Grow…</button>}
                      {v.state !== "attached" && (
                        <button type="button" className="btn btn-xs btn-ghost text-error" aria-label={`Delete ${v.incusName}`} onClick={() => { setDone(null); setDeleting(v); }} data-testid="delete-volume">Delete…</button>
                      )}
                    </div>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </TableFrame>
      ))}
      <SnapshotsDialog application={application} volume={snapshotting} onClose={() => setSnapshotting(null)} />
      <GrowVolumeDialog
        application={application}
        volume={growing}
        onClose={() => setGrowing(null)}
        onGrown={(message) => { setDone(message); setGrowing(null); setRound((r) => r + 1); }}
      />
      <DeleteVolumeDialog
        application={application}
        volume={deleting}
        onClose={() => setDeleting(null)}
        onDeleted={() => { setDone(`Volume ${deleting?.incusName ?? ""} deleted.`); setDeleting(null); setRound((r) => r + 1); }}
      />
    </div>
  );
}

/** `onTerminal`: offered to people who may open sessions somewhere; the control plane decides this instance. */
function Instances({ application, status, onTerminal }: { application: string; status: ApplicationStatus; onTerminal?: (t: TerminalTarget) => void }) {
  return (
    <div className="flex flex-col gap-2 py-1">
      <h3 className="text-sm font-semibold">Instances</h3>
      {status.items.length === 0 ? <p className="text-sm text-base-content/70">No instances.</p> : (
      <TableFrame>
        <thead><tr><th>Instance</th><th className="max-sm:hidden">Status</th><th className="max-sm:hidden">Member</th><th>Port check</th>{onTerminal && <th><span className="sr-only">Actions</span></th>}</tr></thead>
        <tbody>
          {status.items.map((i) => (
            <tr key={`${i.project}/${i.instance}`} data-testid="application-instance">
              <td className="font-medium break-words"><a href={instanceHref(i.project, i.instance)} className="link link-hover" data-testid="instance-link" title="Addresses, disks, snapshots and logs">{i.instance}</a><span className="block text-xs font-normal text-base-content/60">{i.module} · {i.project}</span></td>
              <td className="max-sm:hidden"><span className={`status mr-2 ${dot(i)}`} aria-hidden="true" />{i.status}</td>
              <td className="max-sm:hidden">{i.location}</td>
              <td className="text-sm"><span className={`status mr-2 sm:hidden ${dot(i)}`} aria-hidden="true" /><span className="sm:hidden">{i.status === "Running" ? "" : `${i.status}. `}</span>{health(i, status.services.some((x) => x.module === i.module && x.project === i.project))}</td>
              {onTerminal && (
                <td className="text-right">
                  <button type="button" className="btn btn-xs btn-ghost" aria-label={`Terminal in ${i.instance}`} onClick={() => onTerminal({ application, instance: i.instance })} data-testid="terminal-open">Terminal…</button>
                </td>
              )}
            </tr>
          ))}
        </tbody>
      </TableFrame>
      )}
    </div>
  );
}

/**
 * Applications deployed from the Studio: how many of their instances run, whether the ones
 * with a service address pass their port check, the addresses, and their volumes. Changes are
 * made by deploying from the Studio; the one thing done here is deleting a kept volume.
 */
export function ApplicationsView({ me, onAuth }: { me?: WhoAmI | null; onAuth: (m: string) => void }) {
  const [list, reload] = useLoad<ApplicationList>(() => client.get("applications"), onAuth);
  const [statuses, setStatuses] = useState<Statuses>({});
  const [open, setOpen] = useState<string | null>(null);
  const [terminal, setTerminal] = useState<TerminalTarget | null>(null);
  const canSession = holds(me ?? null, "instance.console") || holds(me ?? null, "instance.desktop");
  const operations = useApplicationOperations(reload);
  useEffect(() => {
    if (list.kind !== "ok") return;
    let current = true;
    void Promise.all(
      list.data.items.map(async (a) => {
        try {
          return [a.name, await client.request("application_status", { params: { name: a.name } })] as const;
        } catch (e) {
          return [a.name, { error: messageOf(e) }] as const;
        }
      }),
    ).then((entries) => { if (current) setStatuses(Object.fromEntries(entries)); });
    return () => { current = false; };
  }, [list]);

  return (
    <Section title="Applications" level={1} state={list} testId="applications" onRetry={reload}>
      {(l) => (
        <>
          <SectionHeader title="Applications" level={1} count={l.items.length}>
            <button type="button" className="btn btn-sm btn-ghost" onClick={reload}>Refresh</button>
          </SectionHeader>
          {l.items.length === 0 ? <p className="text-sm text-base-content/70">Nothing deployed yet. Deploy a design from the Studio.</p> : (
            <TableFrame>
              <thead><tr><th>Application</th><th>Instances</th><th className="max-sm:hidden">Service addresses</th><th><span className="sr-only">Actions</span></th><th><span className="sr-only">Details</span></th></tr></thead>
              <tbody>
                {l.items.map((a) => {
                  const s = statuses[a.name];
                  const ok = s && !("error" in s) ? s : null;
                  const sum = ok ? summary(ok.items) : null;
                  const expanded = open === a.name;
                  return (
                    <Fragment key={a.name}>
                      <tr data-testid="application-row">
                        <td className="font-medium">{a.name}<span className="block text-xs font-normal text-base-content/60">{a.latestRevision ? `Revision ${a.latestRevision}` : "No revision"}</span></td>
                        <td data-testid="application-summary">
                          {!s ? <span className="text-base-content/60">Checking…</span>
                            : "error" in s ? <span className="text-error">{s.error}</span>
                            : <><span className={`status mr-2 ${sum!.dot}`} aria-hidden="true" />{sum!.label}<span className="block text-xs text-base-content/60">{ok!.items.filter((i) => i.status === "Running").length} of {ok!.items.length} running</span>{ok!.trafficBlocked && <span role="status" className="mt-1 block max-w-md text-xs text-warning-content bg-warning/15 rounded px-2 py-1" data-testid="traffic-blocked">{ok!.trafficBlocked}</span>}</>}
                        </td>
                        <td className="font-mono text-xs max-sm:hidden">{ok?.services.map((v) => <span key={`${v.project}/${v.module}`} className="block">{v.address} <span className="font-sans text-base-content/60">{v.module}</span></span>) ?? null}{ok && !ok.services.length && <span className="font-sans text-base-content/60">None</span>}</td>
                        <td className="text-right">{a.actions && <ApplicationActions name={a.name} actions={a.actions} control={operations} />}</td>
                        <td className="text-right">
                          <button type="button" className="btn btn-xs btn-ghost" disabled={!ok} aria-expanded={expanded} aria-controls={`details-${a.name}`} onClick={() => setOpen(expanded ? null : a.name)} data-testid="application-toggle">
                            {expanded ? "Hide details" : "Show details"}
                          </button>
                        </td>
                      </tr>
                      {expanded && ok && (
                        <tr id={`details-${a.name}`}><td colSpan={5} className="bg-base-200/40"><div className="flex flex-col gap-4 py-1"><Instances application={a.name} status={ok} onTerminal={canSession ? setTerminal : undefined} /><Volumes application={a.name} /></div></td></tr>
                      )}
                    </Fragment>
                  );
                })}
              </tbody>
            </TableFrame>
          )}
          {list.kind === "ok" && Object.values(statuses).some((s) => "error" in s) && <ErrorPanel message="Some applications' state could not be read." onRetry={reload} />}
          {operations.dialog}
          <TerminalDialog target={terminal} onClose={() => setTerminal(null)} onAuth={onAuth} />
        </>
      )}
    </Section>
  );
}
