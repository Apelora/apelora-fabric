import { useEffect } from "react";

import type { JobList } from "@apelora/api-client";

import { JobStatus, RequestId, isActive, jobTarget, kindLabel, useJobControl } from "../components/jobs";
import { Section, SectionHeader, TableFrame } from "../components/ui";
import { client, useLoad } from "../lib/api";
import { jobHref } from "../lib/routes";

/** Jobs that are still pending or running make the list refresh itself every few seconds. */
const busy = (l: JobList) => l.items.some(isActive);

export function OperationsView({ onAuth }: { onAuth: (m: string) => void }) {
  const [jobs, reload] = useLoad<JobList>(() => client.get("jobs"), onAuth);
  const active = jobs.kind === "ok" && busy(jobs.data);
  useEffect(() => {
    if (!active) return;
    const t = setInterval(reload, 3000);
    return () => clearInterval(t);
  }, [active, reload]);
  const control = useJobControl(() => reload());

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
              <thead><tr><th>Requested</th><th>Change</th><th className="max-sm:hidden">Target</th><th>Status</th><th className="max-sm:hidden">Attempts</th><th><span className="sr-only">Actions</span></th></tr></thead>
              <tbody>
                {l.items.map((j) => {
                  const err = control.error?.id === j.id ? control.error.message : null;
                  const showRetry = j.status === "failed" && !!j.actions?.retry.allowed;
                  const showCancel = isActive(j) && !!j.actions?.cancel.allowed;
                  return (
                    <tr key={j.id} data-testid="job-row">
                      <td className="whitespace-nowrap">{new Date(j.createdAt).toLocaleString()}</td>
                      <td>
                        <a href={jobHref(j.id)} className="link link-hover font-medium" data-testid="job-link">{kindLabel(j.kind)}</a>
                        <span className="mt-1 block font-mono text-xs sm:hidden">{jobTarget(j)}</span>
                        {j.requestId && <span className="mt-1 block"><RequestId id={j.requestId} /></span>}
                      </td>
                      <td className="font-mono text-xs max-sm:hidden">{jobTarget(j)}</td>
                      <td>
                        <JobStatus job={j} />
                        {j.lastError && <span className="mt-1 block text-xs text-error" data-testid="job-error">{j.lastError}</span>}
                        {err && <span role="alert" className="mt-1 block text-xs text-error" data-testid="job-action-error">{err}</span>}
                      </td>
                      <td className="max-sm:hidden">{j.attempts} of {j.maxAttempts}</td>
                      <td className="text-right">
                        <span className="inline-flex flex-wrap justify-end gap-1">
                          {showRetry && <button type="button" className="btn btn-xs btn-ghost" aria-label={`Retry ${kindLabel(j.kind).toLowerCase()} ${jobTarget(j)}`} disabled={control.busy === j.id} onClick={() => void control.retry(j)} data-testid="job-retry">Retry</button>}
                          {showCancel && <button type="button" className="btn btn-xs btn-ghost text-error" aria-label={`Cancel ${kindLabel(j.kind).toLowerCase()} ${jobTarget(j)}`} disabled={control.busy === j.id} onClick={() => control.askCancel(j)} data-testid="job-cancel">Cancel…</button>}
                        </span>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </TableFrame>
          )}
          {control.dialog}
        </>
      )}
    </Section>
  );
}
