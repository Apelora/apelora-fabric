import { useEffect, useMemo, type ReactNode } from "react";

import type { JobAttempt, JobDetail } from "@apelora/api-client";

import { JobStatus, RequestId, STATUS, isActive, kindLabel, useJobControl } from "../components/jobs";
import { Badge, ErrorPanel, Notice, TableFrame, sentence, type Tone } from "../components/ui";
import { client, useLoad } from "../lib/api";
import { instanceHref, jobHref } from "../lib/routes";

const when = (iso?: string | null) => (iso ? new Date(iso).toLocaleString() : "—");

/** "4 s", "2 min 5 s", from two RFC 3339 times. */
export function duration(from?: string | null, to?: string | null): string {
  if (!from || !to) return "—";
  const s = Math.max(0, Math.round((Date.parse(to) - Date.parse(from)) / 1000));
  if (s < 60) return `${s} s`;
  const m = Math.floor(s / 60);
  return m < 60 ? `${m} min ${s % 60} s` : `${Math.floor(m / 60)} h ${m % 60} min`;
}

const OUTCOME: Record<string, { label: string; tone: Tone }> = {
  succeeded: { label: "Succeeded", tone: "ok" },
  failed: { label: "Failed", tone: "error" },
  rejected: { label: "Refused", tone: "error" },
  waiting: { label: "Waiting on its steps", tone: "info" },
  cancelled: { label: "Cancelled", tone: "neutral" },
  lost: { label: "Worker stopped; taken over", tone: "warn" },
};

const RELATION: Record<string, string> = { "retry-of": "Retries", "retried-by": "Retried by", "same-request": "Same request" };

function Fact({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-1">
      <dt className="text-xs text-base-content/60">{label}</dt>
      <dd className="text-sm break-words">{children}</dd>
    </div>
  );
}

function Outcome({ a }: { a: JobAttempt }) {
  const o = a.outcome ? OUTCOME[a.outcome] ?? { label: a.outcome, tone: "neutral" as Tone } : { label: "Running", tone: "info" as Tone };
  return <Badge tone={o.tone}>{o.label}</Badge>;
}

/** A job on its own page: what it is, where it stands, every attempt and the jobs linked to it. */
export function JobDetailView({ id, onAuth }: { id: string; onAuth: (m: string) => void }) {
  const [detail, reload] = useLoad<JobDetail>(() => client.request("job_detail", { params: { id } }), onAuth);
  const active = detail.kind === "ok" && isActive(detail.data.job);
  useEffect(() => {
    if (!active) return;
    const t = setInterval(reload, 3000);
    return () => clearInterval(t);
  }, [active, reload]);
  const control = useJobControl((retried) => { if (retried) window.location.hash = jobHref(retried); else reload(); });
  const operations = useMemo(() => (detail.kind === "ok" ? detail.data.job.incusOperations : []), [detail]);

  return (
    <div className="flex flex-col gap-6" data-testid="job-detail">
      <nav aria-label="Breadcrumb" className="breadcrumbs py-0 text-sm">
        <ul><li><a href="#/operations" className="pointer-coarse:min-h-11">Operations</a></li><li aria-current="page">Job</li></ul>
      </nav>
      {detail.kind === "loading" && <p role="status" className="text-sm text-base-content/60">Loading…</p>}
      {detail.kind === "error" && (
        <>
          <h1 className="text-lg font-semibold">Job</h1>
          <ErrorPanel message={detail.message} testId="job-detail-error" onRetry={reload} />
        </>
      )}
      {detail.kind === "ok" && (() => {
        const d = detail.data;
        const j = d.job;
        const instance = j.request as { project?: unknown; name?: unknown };
        const instanceLink = typeof instance.project === "string" && typeof instance.name === "string" && j.kind !== "instance.delete"
          ? instanceHref(instance.project, instance.name) : null;
        const err = control.error?.id === j.id ? control.error.message : null;
        const relevantRetry = j.status === "failed";
        const relevantCancel = isActive(j) && !j.cancelRequestedAt;
        return (
          <>
            <header className="flex flex-wrap items-start justify-between gap-3">
              <div className="flex min-w-0 flex-col gap-1">
                <h1 className="text-lg font-semibold">{kindLabel(j.kind)} <span className="font-mono text-base font-normal break-all">{d.target}</span></h1>
                <div className="flex flex-wrap items-center gap-2 text-sm text-base-content/70">
                  <JobStatus job={j} />
                  <span>Attempt {j.attempts} of {j.maxAttempts}</span>
                </div>
              </div>
              {(relevantRetry || relevantCancel) && (
                <div className="flex flex-col items-end gap-1">
                  <div className="flex flex-wrap justify-end gap-2">
                    {relevantRetry && (
                      <button type="button" className="btn btn-sm btn-primary" disabled={!j.actions.retry.allowed || control.busy === j.id} title={j.actions.retry.reason ?? undefined} onClick={() => void control.retry(j)} data-testid="job-retry">
                        {control.busy === j.id ? "Queuing…" : "Retry"}
                      </button>
                    )}
                    {relevantCancel && (
                      <button type="button" className="btn btn-sm btn-ghost text-error" disabled={!j.actions.cancel.allowed || control.busy === j.id} title={j.actions.cancel.reason ?? undefined} onClick={() => control.askCancel(j)} data-testid="job-cancel">
                        Cancel…
                      </button>
                    )}
                  </div>
                  {relevantRetry && !j.actions.retry.allowed && j.actions.retry.reason && <p className="max-w-xs text-right text-xs text-base-content/70" data-testid="job-retry-reason">{j.actions.retry.reason}</p>}
                  {relevantCancel && !j.actions.cancel.allowed && j.actions.cancel.reason && <p className="max-w-xs text-right text-xs text-base-content/70" data-testid="job-cancel-reason">{j.actions.cancel.reason}</p>}
                </div>
              )}
            </header>

            {err && <ErrorPanel message={sentence(err)} testId="job-action-error" />}
            {j.lastError && (
              <Notice tone={j.status === "failed" ? "error" : "warn"} title={j.status === "failed" ? "Why it failed" : "Last attempt's error"} testId="job-last-error">
                <span className="break-words">{j.lastError}</span>
                {j.status === "failed" && j.actions.retry.allowed && <span className="block text-base-content/70">Retry starts a new job; this one is kept.</span>}
              </Notice>
            )}
            {j.cancelRequestedAt && (
              <Notice tone="info" title={j.status === "cancelled" ? "Cancelled" : "Cancellation requested"} testId="job-cancel-note">
                Asked by {j.cancelRequestedBy ?? "—"} at {when(j.cancelRequestedAt)}. {j.status === "cancelled" ? "" : "It stops at its next safe point: "}{d.safePoints}
              </Notice>
            )}

            <section aria-labelledby="job-facts" className="flex flex-col gap-2">
              <h2 id="job-facts" className="font-semibold">Details</h2>
              <dl className="grid grid-cols-1 gap-x-6 gap-y-3 rounded-box border border-base-300 p-4 sm:grid-cols-2 lg:grid-cols-3">
                <Fact label="Queued by">{j.queuedBy ?? (j.requestId ? "—" : "Apelora (supervisor or schedule)")}</Fact>
                <Fact label="Requested">{when(j.createdAt)}</Fact>
                <Fact label="Started">{when(d.startedAt)}</Fact>
                <Fact label="Finished">{when(d.finishedAt)}</Fact>
                <Fact label="Took">{duration(d.startedAt, d.finishedAt)}</Fact>
                <Fact label="Target">{instanceLink ? <a className="link font-mono" href={instanceLink}>{d.target}</a> : <span className="font-mono">{d.target}</span>}</Fact>
                {d.application && <Fact label="Application">{d.application}</Fact>}
                <Fact label="Request">{j.requestId ? <RequestId id={j.requestId} /> : "None (queued by Apelora itself)"}</Fact>
                {j.retryOf && <Fact label="Retries"><a className="link font-mono text-xs" href={jobHref(j.retryOf)}>{j.retryOf}</a></Fact>}
                {j.retriedBy && <Fact label="Retried by"><a className="link font-mono text-xs" href={jobHref(j.retriedBy)}>{j.retriedBy}</a></Fact>}
              </dl>
            </section>

            <section aria-labelledby="job-attempts" className="flex flex-col gap-2">
              <h2 id="job-attempts" className="font-semibold">Attempts <span className="font-normal text-base-content/60">({d.attempts.length})</span></h2>
              {d.attempts.length === 0 ? (
                <p className="text-sm text-base-content/70">{j.attempts > 0 ? "This job ran before attempts were recorded." : "No worker has picked it up yet."}</p>
              ) : (
                <TableFrame>
                  <thead><tr><th>#</th><th>Outcome</th><th>Started</th><th className="max-sm:hidden">Took</th><th className="max-sm:hidden">Worker</th></tr></thead>
                  <tbody>
                    {d.attempts.map((a) => (
                      <tr key={a.attempt} data-testid="attempt-row">
                        <td>{a.attempt}</td>
                        <td>
                          <Outcome a={a} />
                          {a.error && <span className="mt-1 block text-xs break-words text-base-content/80">{a.error}</span>}
                          {a.incusOperations.length > 0 && <span className="mt-1 block text-xs text-base-content/60">Incus {a.incusOperations.length === 1 ? "operation" : "operations"} <code className="font-mono break-all">{a.incusOperations.join(", ")}</code></span>}
                        </td>
                        <td className="sm:whitespace-nowrap">{when(a.startedAt)}</td>
                        <td className="max-sm:hidden">{a.endedAt ? duration(a.startedAt, a.endedAt) : "running"}</td>
                        <td className="font-mono text-xs max-sm:hidden">{a.worker}</td>
                      </tr>
                    ))}
                  </tbody>
                </TableFrame>
              )}
            </section>

            <section aria-labelledby="job-ops" className="flex flex-col gap-2">
              <h2 id="job-ops" className="font-semibold">Incus operations <span className="font-normal text-base-content/60">({operations.length})</span></h2>
              {operations.length === 0 ? <p className="text-sm text-base-content/70">It has not asked Incus to change anything.</p> : (
                <ul className="flex flex-col gap-1 text-sm" data-testid="job-operations">
                  {operations.map((op) => (
                    <li key={op}><code className="font-mono text-xs break-all">{op}</code>{op === j.incusOperation && <span className="ml-2 text-xs text-base-content/70">waiting on it now</span>}</li>
                  ))}
                </ul>
              )}
              {operations.length > 0 && <p className="text-xs text-base-content/60">Incus forgets finished operations; their ids stay here.</p>}
            </section>

            <section aria-labelledby="job-linked" className="flex flex-col gap-2">
              <h2 id="job-linked" className="font-semibold">Linked jobs <span className="font-normal text-base-content/60">({d.linked.length})</span></h2>
              {d.linked.length === 0 ? <p className="text-sm text-base-content/70">No retries, and no other job came from the same request.</p> : (
                <TableFrame>
                  <thead><tr><th>Link</th><th>Change</th><th className="max-sm:hidden">Target</th><th>Status</th></tr></thead>
                  <tbody>
                    {d.linked.map((l) => (
                      <tr key={l.id} data-testid="linked-row">
                        <td>{RELATION[l.relation] ?? l.relation}</td>
                        <td><a className="link link-hover" href={jobHref(l.id)}>{kindLabel(l.kind)}</a></td>
                        <td className="font-mono text-xs max-sm:hidden">{l.target}</td>
                        <td>{STATUS[l.status]?.label ?? l.status}</td>
                      </tr>
                    ))}
                  </tbody>
                </TableFrame>
              )}
            </section>
            {control.dialog}
          </>
        );
      })()}
    </div>
  );
}
