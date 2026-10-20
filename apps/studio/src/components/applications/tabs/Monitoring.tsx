import { useEffect, useState } from "react";

import { useBackend } from "../../../features/applications/backend";
import type { AppDetailView, AuditItem } from "../../../features/applications/model";
import { timeAgo } from "../common";

export function Monitoring({ detail }: { detail: AppDetailView }) {
  const backend = useBackend((s) => s.backend);
  const [events, setEvents] = useState<AuditItem[] | null>(null);
  const [eventsError, setEventsError] = useState<string | null>(null);
  const name = detail.application.name;
  useEffect(() => {
    let live = true;
    backend.events(name).then((e) => { if (live) setEvents(e); }, (e: unknown) => { if (live) setEventsError(e instanceof Error ? e.message : String(e)); });
    return () => { live = false; };
  }, [backend, name, detail.operations.length]);
  const d = detail.application.deployment;
  const rows = detail.appliances.flatMap((a) => a.instances.map((i) => ({ appliance: a.appliance, ...i })));
  return (
    <div className="flex flex-col gap-5" data-testid="tab-monitoring">
      <p className="text-xs text-base-content/60" data-testid="observed-at">
        {d.source === "none" ? "This browser cannot observe a cluster; nothing is monitored." : `Observed ${timeAgo(d.observedAt)}${d.source === "mock" ? " from mock data, not a real cluster" : " from Incus"}.`}
      </p>
      {d.stale && d.source !== "none" && <div role="status" className="rounded-box border border-warning/50 bg-warning/10 px-3 py-2 text-xs" data-testid="monitoring-stale">The control plane could not observe Incus. The table below is the last known state and is not current.</div>}
      <section className="flex flex-col gap-2">
        <h2 className="text-sm font-semibold">Replicas</h2>
        {rows.length === 0 ? (
          <p className="rounded-box border border-dashed border-base-300 px-4 py-6 text-center text-xs text-base-content/60" data-testid="no-replicas">No instances are known for this application.</p>
        ) : (
          <div className="overflow-x-auto rounded-box border border-base-300">
            <table className="table table-sm" data-testid="replica-table">
              <thead><tr><th>Appliance</th><th>Instance</th><th>Status</th><th>Location</th><th>Health</th></tr></thead>
              <tbody>{rows.map((r) => (
                <tr key={r.instance} className={r.unhealthy ? "bg-warning/10" : ""} data-testid="replica-row" data-unhealthy={r.unhealthy}>
                  <td>{r.appliance}</td><td className="font-mono text-xs">{r.instance}</td><td>{r.status}</td><td>{r.location || "—"}</td><td>{r.unhealthy ? "Unhealthy" : "Healthy"}</td>
                </tr>
              ))}</tbody>
            </table>
          </div>
        )}
      </section>
      <section className="flex flex-col gap-2">
        <h2 className="text-sm font-semibold">Recent events</h2>
        {eventsError ? <p role="alert" className="text-xs text-error">{eventsError}</p> : events === null ? <p className="text-xs text-base-content/60">Loading…</p> : events.length === 0 ? (
          <p className="text-xs text-base-content/60" data-testid="no-events">No events recorded.</p>
        ) : (
          <ul className="flex flex-col gap-1 text-xs" data-testid="events">{events.slice(0, 20).map((e) => <li key={e.id} className="flex flex-wrap gap-2"><span className="text-base-content/60">{timeAgo(e.at)}</span><span className="font-medium">{e.action}</span><span>{e.outcome}</span><span className="text-base-content/60">{e.actor}</span></li>)}</ul>
        )}
      </section>
      <p className="rounded-box border border-dashed border-base-300 px-4 py-3 text-xs text-base-content/60" data-testid="no-metrics">CPU, memory and network metrics are not collected yet. This page shows instance status and health only.</p>
    </div>
  );
}
