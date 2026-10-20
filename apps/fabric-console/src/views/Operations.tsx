import { useEffect } from "react";

import type { Job, JobList } from "@apelora/api-client";

import { Section, SectionHeader, TableFrame } from "../components/ui";
import { client, useLoad } from "../lib/api";

const KIND: Record<string, string> = {
  "instance.create": "Create instance",
  "instance.start": "Start instance",
  "instance.stop": "Stop instance",
  "instance.delete": "Delete instance",
};

const STATUS: Record<string, { label: string; dot: string }> = {
  pending: { label: "Waiting", dot: "status-neutral" },
  running: { label: "Running", dot: "status-info" },
  succeeded: { label: "Done", dot: "status-success" },
  failed: { label: "Failed", dot: "status-error" },
  cancelled: { label: "Cancelled", dot: "status-neutral" },
};

const target = (j: Job): string => {
  const r = j.request as { project?: unknown; name?: unknown };
  return typeof r.project === "string" && typeof r.name === "string" ? `${r.project}/${r.name}` : "—";
};

/** Jobs that are still pending or running make the list refresh itself every few seconds. */
const busy = (l: JobList) => l.items.some((j) => j.status === "pending" || j.status === "running");

export function OperationsView({ onAuth }: { onAuth: (m: string) => void }) {
  const [jobs, reload] = useLoad<JobList>(() => client.get("jobs"), onAuth);
  const active = jobs.kind === "ok" && busy(jobs.data);
  useEffect(() => {
    if (!active) return;
    const t = setInterval(reload, 3000);
    return () => clearInterval(t);
  }, [active, reload]);

  return (
    <Section title="Operations" level={1} state={jobs} testId="operations" onRetry={reload}>
      {(l) => (
        <>
          <SectionHeader title="Operations" level={1} count={l.items.length}>
            <button type="button" className="btn btn-sm btn-ghost" onClick={reload}>Refresh</button>
          </SectionHeader>
          <p className="text-sm text-base-content/70">
            Changes the worker carries out, newest first.{active ? " Updates every few seconds while some are in progress." : ""}
          </p>
          {l.items.length === 0 ? <p className="text-sm text-base-content/70">Nothing has been requested yet.</p> : (
            <TableFrame>
              <thead><tr><th>Requested</th><th>Change</th><th>Instance</th><th>Status</th><th className="max-sm:hidden">Attempts</th></tr></thead>
              <tbody>
                {l.items.map((j) => {
                  const s = STATUS[j.status] ?? { label: j.status, dot: "status-neutral" };
                  return (
                    <tr key={j.id} data-testid="job-row">
                      <td className="whitespace-nowrap">{new Date(j.createdAt).toLocaleString()}</td>
                      <td>{KIND[j.kind] ?? j.kind}</td>
                      <td className="font-mono text-xs">{target(j)}</td>
                      <td>
                        <span className={`status ${s.dot} mr-2`} aria-hidden="true" />{s.label}
                        {j.lastError && <span className="mt-1 block text-xs text-error" data-testid="job-error">{j.lastError}</span>}
                      </td>
                      <td className="max-sm:hidden">{j.attempts} of {j.maxAttempts}</td>
                    </tr>
                  );
                })}
              </tbody>
            </TableFrame>
          )}
        </>
      )}
    </Section>
  );
}
