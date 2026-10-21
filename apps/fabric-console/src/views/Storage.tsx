import { useEffect, useRef, useState, type FormEvent } from "react";

import type {
  Job, OrphanList, RestorePlan, ScheduleState, StoragePoolReport, StorageVolume, StorageVolumeList, VolumeBackupItem, VolumeBackupList, WhoAmI,
} from "@apelora/api-client";

import { Badge, ErrorPanel, FormDialog, Notice, Section, SectionHeader, TableFrame, sentence, type Tone } from "../components/ui";
import { JobStatus } from "../components/jobs";
import { client, followJob, idempotencyKey, messageOf, useLoad } from "../lib/api";
import { fmtTime } from "../lib/identity";
import { can } from "../lib/permissions";
import { jobHref } from "../lib/routes";
import { size } from "./Applications";
import { Meter, bytes } from "./Health";

/** "every hour", "every 6 hours", "every day", "every 15 minutes". */
export function every(minutes: number): string {
  if (minutes % 10080 === 0) return minutes === 10080 ? "every week" : `every ${minutes / 10080} weeks`;
  if (minutes % 1440 === 0) return minutes === 1440 ? "every day" : `every ${minutes / 1440} days`;
  if (minutes % 60 === 0) return minutes === 60 ? "every hour" : `every ${minutes / 60} hours`;
  return `every ${minutes} minutes`;
}

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

function scheduleText(kind: "Snapshots" | "Backups", s: ScheduleState | null | undefined): string {
  return s ? `${kind} ${every(s.everyMinutes)}, keeping ${s.keep}` : `${kind}: not scheduled`;
}

// ---- pools --------------------------------------------------------------------------------------

function Pools({ onAuth }: { onAuth: (m: string) => void }) {
  const [state, reload] = useLoad<StoragePoolReport>(() => client.get("storage_pool_usage"), onAuth);
  return (
    <Section title="Storage pools" state={state} testId="storage-pools" onRetry={reload}>
      {(r) => (
        <>
          <SectionHeader title="Storage pools" count={r.items.length} source={r.source} />
          {/* A list rather than a table: on phones each pool's meters take the full width. */}
          <ul className="divide-y divide-base-300 rounded-box border border-base-300">
              {r.items.map((p) => (
                <li key={p.name} className="grid gap-3 px-4 py-3 text-sm sm:grid-cols-[minmax(0,1fr)_minmax(0,2fr)]" data-testid="storage-pool-row">
                  <div>
                    <span className="font-medium">{p.name}</span>
                    <span className="block text-xs text-base-content/70">
                      {[p.role === "volumes" ? "Application volumes" : p.role === "backups" ? "Backups" : null, p.shared ? "Shared by every server" : null, p.status !== "Created" ? p.status : null].filter(Boolean).join(", ") || "Not used by Apelora"}
                    </span>
                    <span className="block text-xs text-base-content/70">Driver {p.driver}</span>
                  </div>
                  <div>
                    {p.total ? (
                      <div className="flex flex-col gap-2">
                        <Meter label={p.shared || p.members.length < 2 ? "Used" : "All servers"} used={p.total.usedBytes} total={p.total.totalBytes} text={`${bytes(p.total.usedBytes)} of ${bytes(p.total.totalBytes)}`} testId="storage-pool-total" />
                        {!p.shared && p.members.length > 1 && p.members.map((m) => (
                          <Meter key={m.member} label={m.member} used={m.usedBytes} total={m.totalBytes} text={`${bytes(m.usedBytes)} of ${bytes(m.totalBytes)}`} testId="storage-pool-member" />
                        ))}
                      </div>
                    ) : <span className="text-sm text-base-content/70">No server reported its space</span>}
                    {p.unreadable.length > 0 && <p className="mt-1 text-xs text-base-content/70">Not read: {p.unreadable.join("; ")}</p>}
                  </div>
                </li>
              ))}
          </ul>
        </>
      )}
    </Section>
  );
}

// ---- volumes ------------------------------------------------------------------------------------

const volumeState = (v: StorageVolume): { label: string; tone: Tone } =>
  v.state === "attached" ? { label: `In use by ${v.usedBy.join(", ")}`, tone: "ok" }
    : v.state === "retained" ? { label: "Kept, not in use", tone: "neutral" }
    : v.state === "missing" ? { label: "Not found in Incus", tone: "error" }
    : { label: "Not read from Incus", tone: "neutral" };

function backupKind(b: VolumeBackupItem): string {
  if (b.beforeRestore) return "Safety copy before a restore";
  return b.scheduled ? "Scheduled" : "On request";
}

type Target = { application: string; volume: string; name: string; users: string[] };

/** Follows one job to its end and reports it; returns the final job. */
function useFollow() {
  const [job, setJob] = useState<Job | null>(null);
  const [error, setError] = useState<string | null>(null);
  const follow = (id: string) => new Promise<Job>((resolve) => {
    setError(null);
    followJob(id, (j) => { setJob(j); if (j.status !== "pending" && j.status !== "running") resolve(j); })
      .catch((e: unknown) => setError(messageOf(e)));
  });
  return { job, setJob, error, follow };
}

/** The restore step of the backups dialog: the server's dry run, then the volume's name typed. */
function RestoreStep({ target, backup, onBack, onDone }: { target: Target; backup: VolumeBackupItem; onBack: () => void; onDone: (message: string) => void }) {
  const [safety, setSafety] = useState(true);
  const [plan, setPlan] = useState<RestorePlan | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [typed, setTyped] = useState("");
  const [busy, setBusy] = useState(false);
  const { job, error: followError, follow } = useFollow();
  const params = { name: target.application, volume: target.volume, backup: backup.name };
  useEffect(() => {
    let current = true;
    setPlan(null);
    setError(null);
    client.request("preview_volume_restore", { params, query: { safetyBackup: safety } }).then(
      (p) => { if (current) setPlan(p.plan); },
      (e: unknown) => { if (current) setError(messageOf(e)); },
    );
    return () => { current = false; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [target.application, target.volume, backup.name, safety]);
  const blocked = plan ? plan.blocked.length > 0 : true;
  const start = async () => {
    setBusy(true);
    setError(null);
    try {
      const accepted = await client.request("restore_volume_backup", { params, body: { confirm: typed, safetyBackup: safety }, idempotencyKey: idempotencyKey() });
      const final = await follow(accepted.jobId);
      if (final.status === "succeeded") {
        onDone(`Volume ${target.volume} now holds the data from ${fmtTime(backup.takenAt)}.${target.users.length ? ` Start ${target.users.join(", ")} again once the data is checked.` : ""}`);
      }
    } catch (e) {
      setError(messageOf(e));
    } finally {
      setBusy(false);
    }
  };
  const running = !!job && (job.status === "pending" || job.status === "running");
  return (
    <div className="flex flex-col gap-3" data-testid="restore-step">
      <p className="text-sm">
        Data written since {fmtTime(backup.takenAt)} is replaced. {safety ? "Only the safety backup can bring it back." : "Nothing can bring it back."}
      </p>
      {!plan && !error && <p role="status" className="text-sm text-base-content/70">Checking what the restore replaces…</p>}
      {plan && (
        <>
          {plan.blocked.length > 0 && (
            <Notice tone="warn" title="It cannot run now" testId="restore-blocked">
              <ul className="list-disc pl-5">{plan.blocked.map((b) => <li key={b}>{sentence(b)}</li>)}</ul>
            </Notice>
          )}
          <div className="flex flex-col gap-1">
            <h4 className="text-sm font-semibold">What happens</h4>
            <ol className="list-decimal pl-5 text-sm" data-testid="restore-steps">{plan.steps.map((s) => <li key={s}>{s}</li>)}</ol>
          </div>
          {plan.snapshotsDeleted.length > 0 && (
            <p className="text-sm" data-testid="restore-snapshots">Snapshots deleted: <span className="font-mono">{plan.snapshotsDeleted.join(", ")}</span></p>
          )}
        </>
      )}
      <label className="flex items-start gap-2 text-sm">
        <input type="checkbox" className="checkbox checkbox-sm mt-0.5" checked={safety} onChange={(e) => setSafety(e.target.checked)} disabled={busy || running} data-testid="restore-safety" />
        <span>Take a safety backup of the current data first</span>
      </label>
      <label className="flex flex-col gap-1 text-sm">
        <span>Type <strong className="font-mono">{target.volume}</strong> to confirm</span>
        <input className="input input-sm w-full font-mono" autoComplete="off" spellCheck={false} value={typed} onChange={(e) => setTyped(e.target.value)} disabled={busy || running} data-testid="restore-confirm-input" />
      </label>
      {job && (
        <p className="flex flex-wrap items-center gap-2 text-sm" role="status" data-testid="restore-job">
          <JobStatus job={job} /> <span>Restore job</span> <a className="link" href={jobHref(job.id)}>Open the job</a>
          {job.lastError && <span className="w-full text-error">{sentence(job.lastError)}</span>}
        </p>
      )}
      {(error ?? followError) && <ErrorPanel message={sentence((error ?? followError)!)} testId="restore-error" />}
      <div className="modal-action mt-0">
        <button type="button" className="btn btn-sm btn-outline" onClick={onBack} disabled={busy && !job}>Back to backups</button>
        <button type="button" className="btn btn-sm btn-ghost text-error" disabled={blocked || typed !== target.volume || busy || running} onClick={() => void start()} data-testid="restore-start">
          {busy || running ? "Restoring…" : "Restore volume"}
        </button>
      </div>
    </div>
  );
}

/** A volume's backups: back up now, and restore from one of them. */
function BackupsDialog({ target, canOperate, onClose, onChanged }: { target: Target | null; canOperate: boolean; onClose: () => void; onChanged: () => void }) {
  const [list, setList] = useState<VolumeBackupList | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [restoring, setRestoring] = useState<VolumeBackupItem | null>(null);
  const [round, setRound] = useState(0);
  const [busy, setBusy] = useState(false);
  const { job, setJob, follow } = useFollow();
  const last = useRef<Target | null>(null);
  if (target) last.current = target;
  const shown = target ?? last.current;
  useEffect(() => { setNotice(null); setRestoring(null); setJob(null); }, [target, setJob]);
  useEffect(() => {
    if (!target) return;
    let current = true;
    setError(null);
    client.request("volume_backups", { params: { name: target.application, volume: target.volume } }).then(
      (l) => { if (current) setList(l); },
      (e: unknown) => { if (current) setError(messageOf(e)); },
    );
    return () => { current = false; };
  }, [target, round]);
  const backUp = async () => {
    if (!shown) return;
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      const accepted = await client.request("back_up_volume", { params: { name: shown.application, volume: shown.volume }, idempotencyKey: idempotencyKey() });
      const final = await follow(accepted.jobId);
      if (final.status === "succeeded") setNotice("Backup made.");
      setRound((r) => r + 1);
      onChanged();
    } catch (e) {
      setError(messageOf(e));
    } finally {
      setBusy(false);
    }
  };
  const items = list?.items ?? [];
  return (
    <FormDialog open={!!target} title={restoring ? `Restore ${shown?.volume ?? ""} from a backup` : `Backups of ${shown?.volume ?? ""}`} onClose={onClose} testId="backups-dialog" wide>
      {!!target && shown && restoring ? (
        <RestoreStep target={shown} backup={restoring} onBack={() => setRestoring(null)} onDone={(m) => { setRestoring(null); setNotice(m); setRound((r) => r + 1); onChanged(); }} />
      ) : !!target && shown && (
        <>
          <p className="text-sm text-base-content/80">Copies in another pool, on another server where there is one, so they survive losing this server.</p>
          {list && !list.backupPool && <Notice tone="info" testId="backups-off">Backups are off: no backup pool is set (APELORA_BACKUP_POOL).</Notice>}
          {list && (
            <p className="text-sm text-base-content/70" data-testid="backups-schedule">{scheduleText("Backups", list.schedules.backups)}{list.schedules.backups?.error ? `. Last run failed: ${list.schedules.backups.error}` : ""}</p>
          )}
          {notice && <p className="text-sm" role="status" data-testid="backups-notice">{notice}</p>}
          {job && job.kind === "volume.backup" && job.status !== "succeeded" && (
            <p className="flex flex-wrap items-center gap-2 text-sm" role="status"><JobStatus job={job} /> Backup job <a className="link" href={jobHref(job.id)}>Open the job</a>{job.lastError && <span className="w-full text-error">{sentence(job.lastError)}</span>}</p>
          )}
          {!list && !error && <p className="text-sm text-base-content/70" role="status">Reading backups…</p>}
          {list && (items.length === 0 ? <p className="text-sm text-base-content/70">No backups yet.</p> : (
            <TableFrame>
              <thead><tr><th>Taken</th><th className="max-sm:hidden">Where</th><th className="max-sm:hidden">Size</th>{canOperate && <th><span className="sr-only">Actions</span></th>}</tr></thead>
              <tbody>
                {[...items].reverse().map((b) => (
                  <tr key={b.name} data-testid="backup-row">
                    <td>
                      {fmtTime(b.takenAt)}
                      <span className="block text-xs text-base-content/70">{backupKind(b)}{b.present === false ? ", not found in Incus" : ""}</span>
                      <span className="block font-mono text-xs text-base-content/60 sm:hidden">{b.pool}{b.member ? ` on ${b.member}` : ""}</span>
                    </td>
                    <td className="max-sm:hidden"><span className="font-mono text-xs">{b.name}</span><span className="block text-xs text-base-content/70">{b.pool}{b.member ? ` on ${b.member}` : ""}</span></td>
                    <td className="max-sm:hidden">{size(b.sizeMib)}</td>
                    {canOperate && (
                      <td className="text-right">
                        <button type="button" className="btn btn-xs btn-ghost" aria-label={`Restore from the backup taken ${fmtTime(b.takenAt)}`} disabled={b.present === false || busy} onClick={() => { setNotice(null); setRestoring(b); }} data-testid="backup-restore">Restore…</button>
                      </td>
                    )}
                  </tr>
                ))}
              </tbody>
            </TableFrame>
          ))}
          {error && <ErrorPanel message={sentence(error)} testId="backups-error" />}
          <div className="modal-action mt-0">
            <button type="button" className="btn btn-sm btn-outline" onClick={onClose}>Close</button>
            {canOperate && list?.backupPool && (
              <button type="button" className="btn btn-sm btn-primary" disabled={busy} onClick={() => void backUp()} data-testid="backup-now">{busy ? "Backing up…" : "Back up now"}</button>
            )}
          </div>
        </>
      )}
    </FormDialog>
  );
}

const INTERVALS = [15, 60, 360, 720, 1440, 10080];

function ScheduleFields({ legend, value, onChange, disabled, note, testId }: { legend: string; value: { on: boolean; every: number; keep: number }; onChange: (v: { on: boolean; every: number; keep: number }) => void; disabled?: boolean; note?: string; testId: string }) {
  return (
    <fieldset className="flex flex-col gap-2 rounded-box border border-base-300 p-3" data-testid={testId}>
      <legend className="px-1 text-sm font-semibold">{legend}</legend>
      <label className="flex items-center gap-2 text-sm">
        <input type="checkbox" className="checkbox checkbox-sm" checked={value.on} disabled={disabled} onChange={(e) => onChange({ ...value, on: e.target.checked })} />
        <span>On a schedule</span>
      </label>
      {note && <p className="text-xs text-base-content/70">{note}</p>}
      {value.on && (
        <div className="flex flex-wrap gap-3">
          <label className="flex flex-col gap-1 text-sm">
            <span>How often</span>
            <select className="select select-sm w-40" value={value.every} onChange={(e) => onChange({ ...value, every: Number(e.target.value) })}>
              {[...new Set([...INTERVALS, value.every])].sort((a, b) => a - b).map((m) => <option key={m} value={m}>{sentence(every(m))}</option>)}
            </select>
          </label>
          <label className="flex flex-col gap-1 text-sm">
            <span>Keep the newest</span>
            <input type="number" min={1} max={50} className="input input-sm w-24 tabular-nums" value={value.keep} onChange={(e) => onChange({ ...value, keep: Number(e.target.value) })} />
          </label>
        </div>
      )}
    </fieldset>
  );
}

/** Editing a volume's snapshot and backup schedules. */
function SchedulesDialog({ volume, backupPool, onClose, onSaved }: { volume: StorageVolume | null; backupPool: string | null; onClose: () => void; onSaved: (m: string) => void }) {
  const init = (s: ScheduleState | null | undefined, every: number, keep: number) => ({ on: !!s, every: s?.everyMinutes ?? every, keep: s?.keep ?? keep });
  const [snap, setSnap] = useState(() => init(volume?.schedules.snapshots, 60, 24));
  const [back, setBack] = useState(() => init(volume?.schedules.backups, 1440, 7));
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  if (!volume) return null;
  const valid = (v: { on: boolean; keep: number }) => !v.on || (Number.isInteger(v.keep) && v.keep >= 1 && v.keep <= 50);
  const save = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await client.request("set_volume_schedules", {
        params: { name: volume.application, volume: volume.incusName },
        body: { snapshots: snap.on ? { everyMinutes: snap.every, keep: snap.keep } : null, backups: back.on ? { everyMinutes: back.every, keep: back.keep } : null },
      });
      onSaved(`Schedules of ${volume.incusName} saved. A new schedule first runs within a minute.`);
    } catch (err) {
      setError(messageOf(err));
    } finally {
      setBusy(false);
    }
  };
  return (
    <FormDialog open title={`Schedules of ${volume.incusName}`} onClose={onClose} testId="schedules-dialog">
      <form className="flex flex-col gap-3" onSubmit={(e) => void save(e)}>
        <ScheduleFields legend="Snapshots" value={snap} onChange={setSnap} testId="schedule-snapshots"
          note="In the volume's own pool. They protect against mistakes, not against losing the server." />
        <ScheduleFields legend="Backups" value={back} onChange={setBack} testId="schedule-backups" disabled={!backupPool && !back.on}
          note={backupPool ? `Copies into pool ${backupPool}, on another server where there is one.` : "No backup pool is set (APELORA_BACKUP_POOL), so backups cannot be scheduled."} />
        {error && <ErrorPanel message={sentence(error)} testId="schedules-error" />}
        <div className="modal-action mt-0">
          <button type="button" className="btn btn-sm btn-outline" onClick={onClose}>Cancel</button>
          <button type="submit" className="btn btn-sm btn-primary" disabled={busy || !valid(snap) || !valid(back) || (back.on && !backupPool)} data-testid="schedules-save">{busy ? "Saving…" : "Save schedules"}</button>
        </div>
      </form>
    </FormDialog>
  );
}

function Volumes({ me, onAuth }: { me: WhoAmI | null; onAuth: (m: string) => void }) {
  const [state, reload] = useLoad<StorageVolumeList>(() => client.get("storage_volumes"), onAuth);
  const [backups, setBackups] = useState<Target | null>(null);
  const [scheduling, setScheduling] = useState<StorageVolume | null>(null);
  const [done, setDone] = useState<string | null>(null);
  const canOperate = can(me, "application.operate", "deploy");
  const canDeploy = can(me, "application.deploy", "deploy");
  return (
    <Section title="Application volumes" state={state} testId="storage-volumes" onRetry={reload}>
      {(l) => (
        <>
          <SectionHeader title="Application volumes" count={l.items.length} source={l.source} />
          {done && <p className="text-sm" role="status" data-testid="storage-volumes-notice">{done}</p>}
          {l.incusError && <Notice tone="warn" testId="storage-volumes-incus">Incus could not be read, so the state of each volume is unknown: {l.incusError}</Notice>}
          {l.items.length === 0 ? <p className="text-sm text-base-content/70">No application has a volume. Volumes are added to a module in the Studio.</p> : (
            <TableFrame>
              <thead><tr><th>Volume</th><th className="max-sm:hidden">Protection</th><th><span className="sr-only">Actions</span></th></tr></thead>
              <tbody>
                {l.items.map((v) => {
                  const st = volumeState(v);
                  const failing = v.schedules.snapshots?.error ?? v.schedules.backups?.error;
                  return (
                    <tr key={`${v.application}/${v.incusName}`} data-testid="storage-volume-row">
                      <td className="align-top break-words">
                        <span className="block text-sm font-medium">{v.incusName}</span>
                        <span className="block text-xs">{size(v.sizeMib)}, {v.name} of {v.module} in {v.application}</span>
                        <span className="block text-xs text-base-content/70">Pool {v.pool}{v.member ? ` on ${v.member}` : ""}</span>
                        <span className="mt-1 block"><Badge tone={st.tone} testId="storage-volume-state">{st.label}</Badge></span>
                      </td>
                      <td className="align-top text-sm max-sm:hidden" data-testid="storage-volume-protection">
                        <span className="block">{scheduleText("Snapshots", v.schedules.snapshots)}</span>
                        <span className="block">{scheduleText("Backups", v.schedules.backups)}</span>
                        <span className="block text-xs text-base-content/70">{v.backups === 0 ? "No backups" : `${plural(v.backups, "backup")}, newest ${fmtTime(v.lastBackupAt)}`}</span>
                        {failing && <span className="block text-xs text-error">Last scheduled run failed: {failing}</span>}
                      </td>
                      <td className="align-top text-right">
                        <div className="flex flex-wrap justify-end gap-1">
                          <button type="button" className="btn btn-xs btn-ghost" aria-label={`Backups of ${v.incusName}`} onClick={() => { setDone(null); setBackups({ application: v.application, volume: v.incusName, name: v.name, users: v.usedBy }); }} data-testid="storage-volume-backups">Backups…</button>
                          {canDeploy && <button type="button" className="btn btn-xs btn-ghost" aria-label={`Schedules of ${v.incusName}`} onClick={() => { setDone(null); setScheduling(v); }} data-testid="storage-volume-schedules">Schedules…</button>}
                        </div>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </TableFrame>
          )}
          <BackupsDialog target={backups} canOperate={canOperate} onClose={() => setBackups(null)} onChanged={reload} />
          {scheduling && (
            <SchedulesDialog key={scheduling.incusName} volume={scheduling} backupPool={l.backupPool ?? null} onClose={() => setScheduling(null)}
              onSaved={(m) => { setScheduling(null); setDone(m); reload(); }} />
          )}
        </>
      )}
    </Section>
  );
}

// ---- orphans ------------------------------------------------------------------------------------

function Orphans({ onAuth }: { onAuth: (m: string) => void }) {
  const [state, reload] = useLoad<OrphanList>(() => client.get("orphan_volumes"), onAuth);
  return (
    <Section title="Volumes no application uses" state={state} testId="storage-orphans" onRetry={reload}>
      {(o) => (
        <>
          <SectionHeader title="Volumes no application uses" count={o.items.length} source={o.source} />
          <p className="text-sm text-base-content/70">Projects looked at: {o.projects.join(", ")}. Apelora never deletes these.</p>
          {o.items.length === 0 ? <p className="text-sm text-base-content/70" data-testid="storage-orphans-empty">None. Every custom volume belongs to an application or a backup.</p> : (
            <TableFrame>
              <thead><tr><th>Volume</th><th>Size</th><th className="max-sm:hidden">Why it is listed</th></tr></thead>
              <tbody>
                {o.items.map((v) => (
                  <tr key={`${v.project}/${v.pool}/${v.member}/${v.name}`} data-testid="storage-orphan-row">
                    <td className="align-top break-words">
                      <span className="block text-sm font-medium">{v.name}</span>
                      <span className="block text-xs text-base-content/70">Pool {v.pool}{v.member ? ` on ${v.member}` : ""} in project {v.project}</span>
                      {v.usedBy.length > 0 && <span className="block text-xs">Used by {v.usedBy.join(", ")}</span>}
                      <span className="block text-xs text-base-content/70 sm:hidden">{v.reason}</span>
                    </td>
                    <td className="align-top tabular-nums">{v.sizeMib != null ? size(v.sizeMib) : "—"}</td>
                    <td className="align-top text-sm max-sm:hidden">{v.reason}</td>
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

/**
 * Storage (`#/storage`): pools and how full they are, every application volume with its
 * schedules and backups (back up now, restore from a backup), and the volumes no application
 * uses. Pools and orphans need `cluster.view`; volumes are listed per application the caller may see.
 */
export function StorageView({ me, onAuth }: { me: WhoAmI | null; onAuth: (m: string) => void }) {
  const infra = can(me, "cluster.view", "read-inventory");
  const apps = can(me, "application.view", "read-inventory");
  return (
    <div className="flex flex-col gap-8">
      {infra && <Pools onAuth={onAuth} />}
      {apps && <Volumes me={me} onAuth={onAuth} />}
      {infra && <Orphans onAuth={onAuth} />}
      {!infra && !apps && <p className="text-sm text-base-content/70">Nothing here is visible to you: storage needs permission to see the cluster or an application.</p>}
    </div>
  );
}
