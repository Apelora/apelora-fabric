import { useCallback, useEffect, useState, type ReactNode } from "react";

import type { ActionState, Instance, InstanceLogList, InstanceLogText, InstancePage, Job, WhoAmI } from "@apelora/api-client";

import { JobStatus, kindLabel } from "../components/jobs";
import { Badge, ConfirmDialog, ErrorPanel, SourceNote, TableFrame, sentence } from "../components/ui";
import { client, followJob, idempotencyKey, messageOf, useLoad } from "../lib/api";
import { holds } from "../lib/identity";
import { jobHref } from "../lib/routes";
import { TerminalDialog, type TerminalTarget } from "../components/TerminalDialog";
import { MoveDialog } from "./Instances";

const KIND_LABEL: Record<string, string> = { container: "Container", "virtual-machine": "Virtual machine", unknown: "Unknown" };
const when = (iso?: string | null) => (iso ? new Date(iso).toLocaleString() : "—");

/** 1536 → "1.5 KiB"; binary units, as Incus reports sizes. */
export function bytes(n?: number | null): string {
  if (n === undefined || n === null) return "—";
  const units = ["B", "KiB", "MiB", "GiB", "TiB"];
  let v = n;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) { v /= 1024; i += 1; }
  return `${i === 0 ? v : v.toFixed(1)} ${units[i]}`;
}

function Fact({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-1">
      <dt className="text-xs text-base-content/60">{label}</dt>
      <dd className="text-sm break-words">{children}</dd>
    </div>
  );
}

function Block({ id, title, count, children }: { id: string; title: string; count?: number; children: ReactNode }) {
  return (
    <section aria-labelledby={id} className="flex flex-col gap-2">
      <h2 id={id} className="font-semibold">{title}{count !== undefined && <span className="font-normal text-base-content/60"> ({count})</span>}</h2>
      {children}
    </section>
  );
}

type Change = "start" | "stop" | "restart" | "delete";
const LABEL: Record<Change, string> = { start: "Starting", stop: "Stopping", restart: "Restarting", delete: "Deleting" };

const TAILS = [16, 64, 256, 1024];

/**
 * The instance's logs, read through the control plane: Incus's log files and the console log,
 * the last part only. Nothing is streamed; Refresh reads again.
 */
function Logs({ project, name, allowed, onAuth }: { project: string; name: string; allowed: ActionState; onAuth: (m: string) => void }) {
  const [files, reloadFiles] = useLoad<InstanceLogList | null>(
    () => (allowed.allowed ? client.request("instance_logs", { params: { project, name } }) : Promise.resolve(null)),
    onAuth,
  );
  const [file, setFile] = useState<string>("console");
  const [tail, setTail] = useState(64);
  const [text, setText] = useState<{ kind: "idle" } | { kind: "loading" } | { kind: "ok"; data: InstanceLogText } | { kind: "error"; message: string }>({ kind: "idle" });
  const read = useCallback(() => {
    if (!allowed.allowed) return () => {};
    let current = true;
    setText({ kind: "loading" });
    const req = file === "console"
      ? client.request("instance_console_log", { params: { project, name }, query: { tailKib: tail } })
      : client.request("instance_log", { params: { project, name, file }, query: { tailKib: tail } });
    req.then((data) => { if (current) setText({ kind: "ok", data }); }, (e: unknown) => { if (current) setText({ kind: "error", message: messageOf(e) }); });
    return () => { current = false; };
  }, [allowed.allowed, project, name, file, tail]);
  useEffect(read, [read]);

  if (!allowed.allowed) {
    return <p className="text-sm text-base-content/70" data-testid="logs-not-allowed">{allowed.reason ?? "You may not read its logs."}</p>;
  }
  const names = files.kind === "ok" && files.data ? files.data.files : [];
  const mock = (text.kind === "ok" && text.data.source.provider === "mock") || (files.kind === "ok" && files.data?.source.provider === "mock");
  return (
    <div className="flex flex-col gap-2" data-testid="logs">
      <div className="flex flex-wrap items-end gap-3">
        <label className="flex flex-col gap-1 text-sm">
          <span>Log</span>
          <select className="select select-sm" value={file} onChange={(e) => setFile(e.target.value)} data-testid="log-file">
            <option value="console">Console output</option>
            {names.map((f) => <option key={f} value={f}>{f}</option>)}
          </select>
        </label>
        <label className="flex flex-col gap-1 text-sm">
          <span>Show the last</span>
          <select className="select select-sm" value={tail} onChange={(e) => setTail(Number(e.target.value))} data-testid="log-tail">
            {TAILS.map((t) => <option key={t} value={t}>{t >= 1024 ? `${t / 1024} MiB` : `${t} KiB`}</option>)}
          </select>
        </label>
        <button type="button" className="btn btn-sm btn-ghost" onClick={() => { reloadFiles(); read(); }} data-testid="log-refresh">Refresh</button>
        {mock && <span className="badge badge-sm badge-ghost" data-testid="log-mock">Mock · not read from Incus</span>}
      </div>
      {files.kind === "error" && <ErrorPanel message={`The list of log files could not be read: ${files.message}`} onRetry={reloadFiles} />}
      {text.kind === "loading" && <p role="status" className="text-sm text-base-content/60">Reading…</p>}
      {text.kind === "error" && <ErrorPanel message={sentence(text.message)} testId="log-error" onRetry={read} />}
      {text.kind === "ok" && (
        <>
          <p className="text-xs text-base-content/60" data-testid="log-size">
            {text.data.log.truncated ? `The last ${tail >= 1024 ? `${tail / 1024} MiB` : `${tail} KiB`} of ${bytes(text.data.log.size)}, from the first whole line.` : `All of it: ${bytes(text.data.log.size)}.`}
          </p>
          {text.data.log.content ? (
            <pre className="max-h-96 overflow-auto rounded-box border border-base-300 bg-base-200 p-3 font-mono text-xs leading-relaxed whitespace-pre-wrap break-words" tabIndex={0} aria-label={`${file === "console" ? "Console output" : file} of ${name}`} data-testid="log-text">{text.data.log.content}</pre>
          ) : <p className="text-sm text-base-content/70">It is empty.</p>}
        </>
      )}
    </div>
  );
}

/** One instance on its own page: state, placement, resources, addresses, disks, snapshots, jobs and logs. */
export function InstanceDetailView({ project, name, me, onAuth }: { project: string; name: string; me: WhoAmI | null; onAuth: (m: string) => void }) {
  const [page, reload] = useLoad<InstancePage>(() => client.request("instance_page", { params: { project, name } }), onAuth);
  const [pending, setPending] = useState<{ label: string; job?: Job; error?: string } | null>(null);
  const [confirm, setConfirm] = useState<Change | null>(null);
  const [moving, setMoving] = useState<Instance | null>(null);
  const [terminal, setTerminal] = useState<TerminalTarget | null>(null);

  const track = (label: string, jobId: string) => {
    setPending({ label });
    followJob(jobId, (job) => setPending({ label, job }))
      .then((job) => {
        setPending(job.status === "succeeded" ? null : { label, job, error: job.lastError ?? `The job ${job.status}.` });
        if (job.status === "succeeded" && label === LABEL.delete) window.location.hash = "#/instances";
        else reload();
      })
      .catch((e: unknown) => setPending({ label, error: messageOf(e) }));
  };

  const act = async (action: Change) => {
    setConfirm(null);
    setPending({ label: LABEL[action] });
    try {
      const params = { project, name };
      const res = action === "delete"
        ? await client.request("delete_instance", { params, idempotencyKey: idempotencyKey() })
        : await client.request("change_state", { params, body: { action }, idempotencyKey: idempotencyKey() });
      reload();
      track(LABEL[action], res.jobId);
    } catch (e) {
      setPending({ label: LABEL[action], error: messageOf(e) });
    }
  };

  return (
    <div className="flex flex-col gap-6" data-testid="instance-detail">
      <nav aria-label="Breadcrumb" className="breadcrumbs py-0 text-sm">
        <ul><li><a href="#/instances" className="pointer-coarse:min-h-11">Instances</a></li><li aria-current="page">{name}</li></ul>
      </nav>
      {page.kind === "loading" && <p role="status" className="text-sm text-base-content/60">Loading…</p>}
      {page.kind === "error" && (
        <>
          <h1 className="text-lg font-semibold font-mono">{name}</h1>
          <ErrorPanel message={sentence(page.message)} testId="instance-detail-error" onRetry={reload} />
        </>
      )}
      {page.kind === "ok" && (() => {
        const { instance: i, actions: a, jobs, source } = page.data;
        const busy = !!pending && !pending.error;
        const running = i.status === "Running";
        const asInstance: Instance = { name: i.name, project: i.project, kind: i.kind, status: i.status, location: i.location, architecture: i.architecture, description: i.description, managed: i.managed, owner: i.owner ?? undefined };
        const btn = (key: Change | "move", label: string, cls: string, onClick: () => void, state: ActionState) => (
          <button type="button" className={`btn btn-sm ${cls}`} disabled={!state.allowed || busy} title={state.reason ?? undefined} onClick={onClick} data-testid={`instance-${key}`}>{label}</button>
        );
        const blocked = [a.start, a.stop].find((s) => !s.allowed && s.reason && /managed|may not|configured|job for this/i.test(s.reason));
        const u = i.usage;
        return (
          <>
            <header className="flex flex-wrap items-start justify-between gap-3">
              <div className="flex min-w-0 flex-col gap-1">
                <h1 className="text-lg font-semibold font-mono break-all">{i.name}</h1>
                <div className="flex flex-wrap items-center gap-2 text-sm text-base-content/70">
                  <Badge tone={running ? "ok" : i.status === "Stopped" ? "neutral" : "warn"} testId="instance-status">{busy ? `${pending?.label}…` : i.status}</Badge>
                  <span>{KIND_LABEL[i.kind] ?? i.kind}</span>
                  <span>in {i.project}</span>
                  {i.managed ? <span className="badge badge-sm badge-ghost" title="Created by Apelora">Apelora</span> : <span className="text-xs">Not managed by Apelora</span>}
                  <SourceNote source={source} />
                </div>
              </div>
              <div className="flex flex-col items-end gap-1">
                <div className="flex flex-wrap justify-end gap-2" data-testid="instance-actions">
                  {running
                    ? <>{btn("restart", "Restart…", "btn-primary", () => setConfirm("restart"), a.restart)}{btn("stop", "Stop…", "btn-outline", () => setConfirm("stop"), a.stop)}</>
                    : btn("start", "Start", "btn-primary", () => void act("start"), a.start)}
                  {holds(me, "instance.console") && i.managed && i.owner && (
                    <button type="button" className="btn btn-sm btn-ghost" onClick={() => setTerminal({ application: i.owner!.application, instance: i.name })} data-testid="terminal-open">Terminal…</button>
                  )}
                  {btn("move", "Move…", "btn-ghost", () => setMoving(asInstance), a.move)}
                  {btn("delete", "Delete…", "btn-ghost text-error", () => setConfirm("delete"), a.delete)}
                </div>
                {blocked && <p className="max-w-sm text-right text-xs text-base-content/70" data-testid="instance-actions-reason">{blocked.reason}</p>}
              </div>
            </header>

            {pending?.error && <ErrorPanel message={`${pending.label} did not finish: ${pending.error}`} testId="instance-action-error" />}
            {busy && pending?.job && <p role="status" className="text-sm text-base-content/70">{pending.label}… job {pending.job.status}{pending.job.attempts > 1 ? `, attempt ${pending.job.attempts}` : ""}. <a className="link" href={jobHref(pending.job.id)}>Open the job</a></p>}

            <Block id="instance-overview" title="Overview">
              <dl className="grid grid-cols-1 gap-x-6 gap-y-3 rounded-box border border-base-300 p-4 sm:grid-cols-2 lg:grid-cols-3">
                <Fact label="Member">{i.location || "—"}</Fact>
                <Fact label="Architecture">{i.architecture || "—"}</Fact>
                <Fact label="Application">{page.data.application ? <a className="link" href="#/applications">{page.data.application}</a> : "—"}{i.owner && <span className="text-base-content/60"> · module {i.owner.module}</span>}</Fact>
                <Fact label="Image">{i.image.description ?? ([i.image.os, i.image.release].filter(Boolean).join(" ") || "—")}{i.image.fingerprint && <span className="block font-mono text-xs text-base-content/60 break-all">{i.image.fingerprint.slice(0, 12)}</span>}</Fact>
                <Fact label="Profiles">{i.profiles.join(", ") || "—"}</Fact>
                <Fact label="Created">{when(i.createdAt)}</Fact>
                <Fact label="Running since">{running ? when(i.startedAt) : "Not running"}</Fact>
                {i.description && <Fact label="Description">{i.description}</Fact>}
              </dl>
            </Block>

            <Block id="instance-resources" title="Resources">
              <dl className="grid grid-cols-1 gap-x-6 gap-y-3 rounded-box border border-base-300 p-4 sm:grid-cols-2 lg:grid-cols-4">
                <Fact label="CPU limit">{i.limits.cpu ?? "No limit"}</Fact>
                <Fact label="Memory">{u?.memoryBytes != null ? `${bytes(u.memoryBytes)} used` : "—"}{` · limit ${i.limits.memory ?? "none"}`}</Fact>
                <Fact label="Root disk">{u?.rootDiskBytes != null ? `${bytes(u.rootDiskBytes)} used` : "—"}{` · size ${i.limits.rootDisk ?? "pool default"}`}</Fact>
                <Fact label="Processes">{u?.processes ?? "—"}</Fact>
              </dl>
              {!u && <p className="text-xs text-base-content/60">Incus reports use only while it runs.</p>}
            </Block>

            <Block id="instance-addresses" title="Network addresses" count={i.addresses.length}>
              {i.addresses.length === 0 ? <p className="text-sm text-base-content/70">{running ? "Incus reports no addresses yet." : "None while it is stopped."}</p> : (
                <TableFrame>
                  <thead><tr><th>Address</th><th>Interface</th><th className="max-sm:hidden">Scope</th></tr></thead>
                  <tbody>{i.addresses.map((ad) => <tr key={`${ad.interface}-${ad.address}`}><td className="font-mono text-xs break-all">{ad.address}/{ad.netmask}</td><td>{ad.interface}</td><td className="max-sm:hidden">{ad.scope}</td></tr>)}</tbody>
                </TableFrame>
              )}
            </Block>

            <Block id="instance-disks" title="Disks and volumes" count={i.disks.length}>
              {i.disks.length === 0 ? <p className="text-sm text-base-content/70">Incus reports no disks.</p> : (
                <TableFrame>
                  <thead><tr><th>Device</th><th>Mounted at</th><th className="max-sm:hidden">Pool</th><th className="max-sm:hidden">Volume</th><th>Size</th></tr></thead>
                  <tbody>{i.disks.map((d) => <tr key={d.device}><td className="font-mono text-xs">{d.device}</td><td className="font-mono text-xs">{d.path ?? "block device"}</td><td className="max-sm:hidden">{d.pool ?? "—"}</td><td className="max-sm:hidden font-mono text-xs">{d.volume ?? "—"}</td><td>{d.size ?? "—"}</td></tr>)}</tbody>
                </TableFrame>
              )}
            </Block>

            <Block id="instance-snapshots" title="Snapshots" count={i.snapshots.length}>
              {i.snapshots.length === 0 ? <p className="text-sm text-base-content/70">None. Volume snapshots are on the application's page.</p> : (
                <TableFrame>
                  <thead><tr><th>Name</th><th>Taken</th><th className="max-sm:hidden">Expires</th></tr></thead>
                  <tbody>{i.snapshots.map((s) => <tr key={s.name}><td className="font-mono text-xs">{s.name}{s.stateful && <span className="ml-2 text-base-content/60">with memory</span>}</td><td>{when(s.createdAt)}</td><td className="max-sm:hidden">{s.expiresAt ? when(s.expiresAt) : "Never"}</td></tr>)}</tbody>
                </TableFrame>
              )}
            </Block>

            <Block id="instance-jobs" title="Recent jobs" count={jobs.length}>
              {jobs.length === 0 ? <p className="text-sm text-base-content/70">Apelora has not changed it.</p> : (
                <TableFrame>
                  <thead><tr><th>Requested</th><th>Change</th><th>Status</th></tr></thead>
                  <tbody>{jobs.map((j) => <tr key={j.id} data-testid="instance-job-row"><td className="whitespace-nowrap">{when(j.createdAt)}</td><td><a className="link link-hover" href={jobHref(j.id)}>{kindLabel(j.kind)}</a></td><td><JobStatus job={j} />{j.lastError && <span className="mt-1 block text-xs text-error">{j.lastError}</span>}</td></tr>)}</tbody>
                </TableFrame>
              )}
            </Block>

            <Block id="instance-logs" title="Logs">
              <Logs project={project} name={name} allowed={a.logs} onAuth={onAuth} />
            </Block>

            <ConfirmDialog
              open={confirm === "stop" || confirm === "restart"}
              title={`${confirm === "stop" ? "Stop" : "Restart"} ${i.name}?`}
              body={<p>{confirm === "stop" ? "It shuts down cleanly. What it serves is unavailable until it is started again." : "It shuts down cleanly and starts again. What it serves is unavailable for that time."}</p>}
              action={confirm === "stop" ? "Stop instance" : "Restart instance"}
              dismiss="Keep it running"
              onCancel={() => setConfirm(null)}
              onConfirm={() => { if (confirm) void act(confirm); }}
            />
            <ConfirmDialog
              open={confirm === "delete"}
              title={`Delete ${i.name}?`}
              body={<p>The instance and its root disk are removed from Incus. A running instance is stopped first. This cannot be undone.</p>}
              action="Delete instance"
              typeToConfirm={i.name}
              onCancel={() => setConfirm(null)}
              onConfirm={() => void act("delete")}
            />
            {moving && <MoveDialog instance={moving} onClose={() => setMoving(null)} onQueued={(_t, label, jobId) => track(label, jobId)} />}
            <TerminalDialog target={terminal} onClose={() => setTerminal(null)} onAuth={onAuth} />
          </>
        );
      })()}
    </div>
  );
}
