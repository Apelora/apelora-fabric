import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";

import { ApiRequestError, type HealthCondition, type HealthReport, type HealthServer, type HealthServers, type HealthUsage } from "@apelora/api-client";

import { Badge, ErrorPanel, FormDialog, Notice, SourceNote, TableFrame, sentence, type Tone } from "../components/ui";
import { client, messageOf } from "../lib/api";

/** How often the page reads again while it is visible. */
export const REFRESH_MS = 20_000;

const WORDS: Record<HealthCondition, string> = { healthy: "Healthy", degraded: "Degraded", down: "Down", unknown: "Unknown", "not-set-up": "Not set up" };
const TONES: Record<HealthCondition, Tone> = { healthy: "ok", degraded: "warn", down: "error", unknown: "neutral", "not-set-up": "neutral" };

export function StateBadge({ state, testId }: { state: HealthCondition; testId?: string }) {
  return <Badge tone={TONES[state]} testId={testId}>{WORDS[state]}</Badge>;
}

const GIB = 1024 ** 3;
export const bytes = (n: number) => (n >= GIB ? `${(n / GIB).toFixed(n >= 100 * GIB ? 0 : 1)} GiB` : `${Math.round(n / 1024 ** 2)} MiB`);

/** `45 s`, `3 min`, `2 h 5 min`, `3 d 4 h`. */
export function ago(secs: number): string {
  const s = Math.max(0, Math.round(secs));
  if (s < 60) return `${s} s`;
  if (s < 3600) return `${Math.floor(s / 60)} min`;
  if (s < 86_400) { const m = Math.floor((s % 3600) / 60); return `${Math.floor(s / 3600)} h${m ? ` ${m} min` : ""}`; }
  const h = Math.floor((s % 86_400) / 3600);
  return `${Math.floor(s / 86_400)} d${h ? ` ${h} h` : ""}`;
}

type Polled<T> = { data: T | null; error: string | null; at: number | null; loading: boolean };

/**
 * Reads `fetcher` now and every `every` ms while the page is visible (and at once when it
 * becomes visible again). A failed read keeps the last data and reports the error beside it.
 */
function usePolled<T>(fetcher: () => Promise<T>, every: number, onAuth: (m: string) => void): [Polled<T>, () => void] {
  const [state, setState] = useState<Polled<T>>({ data: null, error: null, at: null, loading: true });
  const latest = useRef(fetcher);
  latest.current = fetcher;
  const live = useRef(true);
  const read = useCallback(() => {
    setState((s) => ({ ...s, loading: true }));
    latest.current()
      .then((data) => { if (live.current) setState({ data, error: null, at: Date.now(), loading: false }); })
      .catch((e: unknown) => {
        if (!live.current) return;
        if (e instanceof ApiRequestError && e.unauthenticated) onAuth(e.message);
        setState((s) => ({ ...s, error: messageOf(e), loading: false }));
      });
  }, [onAuth]);
  useEffect(() => {
    live.current = true;
    read();
    const timer = setInterval(() => { if (document.visibilityState === "visible") read(); }, every);
    const onVisible = () => { if (document.visibilityState === "visible") read(); };
    document.addEventListener("visibilitychange", onVisible);
    return () => { live.current = false; clearInterval(timer); document.removeEventListener("visibilitychange", onVisible); };
  }, [read, every]);
  return [state, read];
}

/** Re-renders every few seconds so "checked … ago" stays true. */
function useNow(every = 5000) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => { const t = setInterval(() => setNow(Date.now()), every); return () => clearInterval(t); }, [every]);
  return now;
}

function Facts({ items }: { items: (string | null | false | undefined)[] }) {
  const shown = items.filter(Boolean) as string[];
  return shown.length ? <p className="text-sm text-base-content/70">{shown.join(", ")}</p> : null;
}

function Row({ name, state, testId, facts, reason, nextStep, children }: { name: string; state: HealthCondition; testId: string; facts: (string | null | false | undefined)[]; reason?: string | null; nextStep?: string | null; children?: ReactNode }) {
  return (
    <li className="flex flex-col gap-1 px-4 py-3" data-testid={testId} data-state={state}>
      <div className="flex flex-wrap items-baseline justify-between gap-x-4 gap-y-1">
        <h3 className="text-sm font-medium">{name}</h3>
        <StateBadge state={state} />
      </div>
      <Facts items={facts} />
      {reason && <p className="text-sm" data-testid={`${testId}-reason`}>{sentence(reason)}{/[.!?]$/.test(reason) ? "" : "."}</p>}
      {nextStep && <p className="text-sm text-base-content/70">Next step: {nextStep}{/[.!?]$/.test(nextStep) ? "" : "."}</p>}
      {children}
    </li>
  );
}

function Components({ report, now }: { report: HealthReport; now: number }) {
  const { controlPlane: cp, database: db, workers, jobQueue: q, incus, networks, storagePools: pools, identityProviders: idp } = report;
  const age = (iso: string) => ago((now - Date.parse(iso)) / 1000);
  return (
    <ul className="divide-y divide-base-300 rounded-box border border-base-300" aria-label="Components" data-testid="health-components">
      <Row name="Control plane" state={cp.state} testId="component-control-plane" reason={cp.reason} nextStep={cp.nextStep}
        facts={[`Version ${cp.version}`, `up ${ago(cp.uptimeSecs)}`, cp.auth === "tokens" ? "sign-in required" : "sign-in off", cp.provider === "mock" ? "mock inventory" : "inventory from Incus"]} />
      <Row name="Database" state={db.state} testId="component-database" reason={db.reason} nextStep={db.nextStep}
        facts={[db.roundTripMs != null && `query ${db.roundTripMs} ms`, db.schemaVersion != null && `schema ${db.schemaVersion} of ${db.expectedSchemaVersion}`]} />
      <Row name="Deployment workers" state={workers.state} testId="component-workers" reason={workers.reason} nextStep={workers.nextStep}
        facts={[workers.items.some((w) => w.state === "running") && `${workers.items.filter((w) => w.state === "running").length} running`]}>
        {workers.items.length > 0 && (
          <ul className="flex flex-col gap-0.5 text-sm text-base-content/70">
            {workers.items.map((w) => (
              <li key={w.id} className="flex flex-wrap items-baseline justify-between gap-x-4" data-testid="worker-row">
                <span className="min-w-0 break-all">{w.id} on {w.host}, version {w.version}</span>
                <span className="tabular-nums">{w.state === "running" ? `heartbeat ${ago(w.secondsSinceSeen)} ago` : w.state === "stopped" ? `stopped ${age(w.lastSeen)} ago` : `lost, last heartbeat ${age(w.lastSeen)} ago`}</span>
              </li>
            ))}
          </ul>
        )}
      </Row>
      <Row name="Job queue" state={q.state} testId="component-job-queue" reason={q.reason} nextStep={q.nextStep}
        facts={q.counts ? [`${q.counts.waiting} waiting`, q.counts.retrying > 0 && `${q.counts.retrying} waiting to retry`, `${q.counts.running} running`, q.counts.stalled > 0 && `${q.counts.stalled} stalled`, `${q.counts.failedLastHour} failed in the last hour`, q.counts.oldestWaitingSecs != null && `oldest waiting ${ago(q.counts.oldestWaitingSecs)}`] : []} />
      <Row name="Incus" state={incus.state} testId="component-incus" reason={incus.reason} nextStep={incus.nextStep}
        facts={[incus.serverVersion ? `Incus ${incus.serverVersion}` : incus.provider === "mock" ? "mock" : null, incus.clustered != null && (incus.clustered ? "clustered" : "single server"), incus.members && `${incus.members.online} of ${incus.members.total} servers online`, incus.roundTripMs != null && `answers in ${incus.roundTripMs} ms`]} />
      <Row name="Networks" state={networks.state} testId="component-networks" reason={networks.reason} nextStep={networks.nextStep}
        facts={[networks.total != null && `${networks.total} managed`]}>
        {networks.problems.length > 0 && <ul className="list-disc pl-5 text-sm">{networks.problems.map((p) => <li key={p}>{p}</li>)}</ul>}
      </Row>
      <Row name="Storage pools" state={pools.state} testId="component-storage-pools" reason={pools.reason} nextStep={pools.nextStep}
        facts={[pools.total != null && `${pools.total} ${pools.total === 1 ? "pool" : "pools"}`]}>
        {pools.problems.length > 0 && <ul className="list-disc pl-5 text-sm">{pools.problems.map((p) => <li key={p}>{p}</li>)}</ul>}
      </Row>
      <Row name="Identity providers" state={idp.state} testId="component-identity" reason={idp.reason} nextStep={idp.nextStep}
        facts={[idp.items.length > 0 && idp.items.map((p) => `${p.name}: ${!p.enabled ? "off" : p.reachable ? "reachable" : "not reachable"}`).join(", "), `API tokens ${idp.apiTokens ? "accepted" : "off"}`]} />
    </ul>
  );
}

function Meter({ label, used, total, text, testId }: { label: string; used: number; total: number; text: string; testId: string }) {
  return (
    <div className="flex flex-col gap-1" data-testid={testId}>
      <div className="flex items-baseline justify-between gap-3 text-sm">
        <span className="min-w-0 truncate">{label}</span>
        <span className="shrink-0 tabular-nums text-base-content/70">{text}</span>
      </div>
      <progress className="progress h-1.5 w-full" value={Math.min(used, total)} max={total} aria-label={`${label}: ${text}`} />
    </div>
  );
}

function Resources({ server }: { server: HealthServer }) {
  const r = server.resources;
  // The server's warnings below say why; without any, say it here.
  if (!r) return server.warnings.length ? null : <p className="text-sm text-base-content/70" data-testid="server-resources-unavailable">No CPU, memory or storage figures.</p>;
  const mem: HealthUsage | null | undefined = r.memory;
  const load = r.loadAverages[0];
  return (
    <div className="flex flex-col gap-3">
      {r.cpuThreads != null && (load !== undefined
        ? <Meter label="CPU" used={load} total={r.cpuThreads} text={`load ${load.toFixed(2)} on ${r.cpuThreads} threads`} testId="server-cpu" />
        : <p className="text-sm" data-testid="server-cpu">CPU <span className="text-base-content/70">{r.cpuThreads} threads (load not reported)</span></p>)}
      {mem && <Meter label="Memory" used={mem.usedBytes} total={mem.totalBytes} text={`${bytes(mem.usedBytes)} of ${bytes(mem.totalBytes)} used`} testId="server-memory" />}
      {r.pools.map((p) => <Meter key={p.name} label={`Storage pool ${p.name}`} used={p.usedBytes} total={p.totalBytes} text={`${bytes(p.usedBytes)} of ${bytes(p.totalBytes)} used`} testId="server-pool" />)}
    </div>
  );
}

function ServerCard({ server, onDetails }: { server: HealthServer; onDetails: () => void }) {
  const running = server.instances.filter((i) => i.status === "Running").length;
  return (
    <li className="flex flex-col gap-3 rounded-box border border-base-300 p-4" data-testid="server-card" data-state={server.state}>
      <div className="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1">
        <h3 className="min-w-0 break-all font-medium">{server.name}</h3>
        <StateBadge state={server.state} />
      </div>
      <Facts items={[server.status, server.roles.length ? server.roles.join(", ") : "no database role", server.architecture, server.held && "held out of placement", server.resources?.uptimeSecs != null && `up ${ago(server.resources.uptimeSecs)}`]} />
      <Resources server={server} />
      {server.warnings.length > 0 && (
        <ul className="flex flex-col gap-1 text-sm" aria-label={`Warnings for ${server.name}`} data-testid="server-warnings">
          {server.warnings.map((w) => <li key={w} className="flex items-start gap-2"><span className="status status-warning mt-1.5 shrink-0" aria-hidden="true" />{w}</li>)}
        </ul>
      )}
      <div className="flex flex-wrap items-center justify-between gap-2 border-t border-base-300 pt-3 text-sm">
        <span className="text-base-content/70">{server.instances.length === 0 ? "No instances" : `${running} of ${server.instances.length} ${server.instances.length === 1 ? "instance" : "instances"} running`}</span>
        <button type="button" className="btn btn-sm btn-ghost" onClick={onDetails} aria-label={`Instances on ${server.name}`}>Instances</button>
      </div>
    </li>
  );
}

function ServerDialog({ server, onClose }: { server: HealthServer | null; onClose: () => void }) {
  return (
    <FormDialog open={!!server} title={server ? `Instances on ${server.name}` : ""} onClose={onClose} wide testId="server-dialog">
      {server && (
        <>
          <p className="text-sm text-base-content/70">Lists the projects Apelora's Incus identity may see.</p>
          {server.instances.length === 0 ? <p className="text-sm text-base-content/70">Nothing runs here in those projects.</p> : (
            <TableFrame>
              <thead><tr><th>Instance</th><th>Status</th><th className="max-sm:hidden">Type</th><th className="max-sm:hidden">Project</th></tr></thead>
              <tbody>
                {server.instances.map((i) => (
                  <tr key={`${i.project}/${i.name}`} data-testid="server-instance-row">
                    <td className="font-medium">{i.name}{i.managed ? "" : <span className="block text-xs font-normal text-base-content/60">Not managed by Apelora</span>}</td>
                    <td>{i.status}</td>
                    <td className="max-sm:hidden">{i.kind === "virtual-machine" ? "Virtual machine" : i.kind === "container" ? "Container" : "Unknown"}</td>
                    <td className="max-sm:hidden">{i.project}</td>
                  </tr>
                ))}
              </tbody>
            </TableFrame>
          )}
          <div className="modal-action mt-0"><button type="button" className="btn btn-sm" onClick={onClose}>Close</button></div>
        </>
      )}
    </FormDialog>
  );
}

function Servers({ state, onRetry }: { state: Polled<HealthServers>; onRetry: () => void }) {
  const [open, setOpen] = useState<string | null>(null);
  const s = state.data;
  return (
    <section className="flex flex-col gap-3" aria-labelledby="servers-title" data-testid="health-servers">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h2 id="servers-title" className="font-semibold">Servers{s && <span className="font-normal text-base-content/60"> ({s.items.length})</span>}</h2>
        {s && <SourceNote source={s.source} />}
      </div>
      {!s && state.loading && <p role="status" className="text-sm text-base-content/60" data-testid="health-servers-loading">Reading each server…</p>}
      {!s && state.error && <ErrorPanel message={state.error} testId="health-servers-error" onRetry={onRetry} />}
      {s && s.items.length === 0 && <p className="text-sm text-base-content/70">Incus reports no servers.</p>}
      {s && s.items.length > 0 && (
        <ul className="grid grid-cols-1 items-start gap-3 md:grid-cols-2">
          {s.items.map((x) => <ServerCard key={x.name} server={x} onDetails={() => setOpen(x.name)} />)}
        </ul>
      )}
      <ServerDialog server={s?.items.find((x) => x.name === open) ?? null} onClose={() => setOpen(null)} />
    </section>
  );
}

export function HealthView({ onAuth }: { onAuth: (m: string) => void }) {
  const [report, readReport] = usePolled<HealthReport>(() => client.get("health_components"), REFRESH_MS, onAuth);
  const [servers, readServers] = usePolled<HealthServers>(() => client.get("health_servers"), REFRESH_MS, onAuth);
  const now = useNow();
  const refresh = () => { setAsked(true); readReport(); readServers(); };
  const r = report.data;
  const at = report.at;
  // Only a read the person asked for shows as busy; background reads stay quiet.
  const [asked, setAsked] = useState(false);
  const busy = asked && (report.loading || servers.loading);
  useEffect(() => { if (!report.loading && !servers.loading) setAsked(false); }, [report.loading, servers.loading]);
  const failed = [report.error, servers.data ? servers.error : null].filter(Boolean) as string[];
  return (
    <div className="flex flex-col gap-6">
      <div className="flex flex-wrap items-center justify-between gap-3" data-testid="health-summary">
        <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-sm">
          {r ? <><span className="font-medium">Overall</span><StateBadge state={r.overall} testId="health-overall" /></> : null}
          <span className="text-base-content/70" data-testid="health-age">
            {at ? `Checked ${ago((now - at) / 1000)} ago; reads again every ${REFRESH_MS / 1000} s` : "Checking…"}
          </span>
        </div>
        <button type="button" className="btn btn-sm" onClick={refresh} disabled={busy} data-testid="health-refresh">{busy ? "Reading…" : "Read now"}</button>
      </div>

      {r && failed.length > 0 && (
        <Notice tone="warn" testId="health-stale" title="Could not read the latest state">
          {sentence(failed[0]!)}. Showing what was read {at ? `${ago((now - at) / 1000)} ago` : "earlier"}.
        </Notice>
      )}

      <section className="flex flex-col gap-3" aria-labelledby="components-title">
        <h2 id="components-title" className="font-semibold">Components</h2>
        {!r && report.loading && <p role="status" className="text-sm text-base-content/60" data-testid="health-loading">Checking each component…</p>}
        {!r && report.error && <ErrorPanel message={report.error} testId="health-error" onRetry={readReport} />}
        {r && <Components report={r} now={now} />}
      </section>

      <Servers state={servers} onRetry={readServers} />
    </div>
  );
}
