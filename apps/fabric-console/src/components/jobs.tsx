import { useState } from "react";

import type { Job } from "@apelora/api-client";

import { client, idempotencyKey, messageOf } from "../lib/api";
import { Badge, ConfirmDialog, type Tone } from "./ui";

export const KIND: Record<string, string> = {
  "instance.create": "Create instance",
  "instance.start": "Start instance",
  "instance.stop": "Stop instance",
  "instance.delete": "Delete instance",
  "instance.restart": "Restart instance",
  "instance.move": "Move instance",
  "network.sync": "Apply network rules",
  "volume.backup": "Back up volume",
  "application.operation": "Application start, stop or restart",
};
export const kindLabel = (kind: string) => KIND[kind] ?? kind;

export const STATUS: Record<string, { label: string; tone: Tone }> = {
  pending: { label: "Waiting", tone: "neutral" },
  running: { label: "Running", tone: "info" },
  succeeded: { label: "Done", tone: "ok" },
  failed: { label: "Failed", tone: "error" },
  cancelled: { label: "Cancelled", tone: "neutral" },
};

/** The job's status in words; a requested cancellation is said, not hidden behind "Running". */
export function JobStatus({ job }: { job: Job }) {
  const s = STATUS[job.status] ?? { label: job.status, tone: "neutral" as Tone };
  const stopping = !!job.cancelRequestedAt && (job.status === "pending" || job.status === "running");
  return <Badge tone={s.tone} testId="job-status">{stopping ? `${s.label} · cancelling` : s.label}</Badge>;
}

export const jobTarget = (j: Job): string => {
  const r = j.request as { project?: unknown; name?: unknown; application?: unknown };
  if (typeof r.project === "string" && typeof r.name === "string") return `${r.project}/${r.name}`;
  return typeof r.application === "string" ? r.application : "—";
};

export const isActive = (j: Job) => j.status === "pending" || j.status === "running";

/** The request that queued the job: the same id is in the request's audit rows and every log line about it. */
export function RequestId({ id }: { id: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <span className="flex flex-wrap items-center gap-x-1 text-xs text-base-content/70" data-testid="job-request">
      Request <code className="font-mono break-all">{id}</code>
      <button type="button" className="btn btn-ghost btn-xs" aria-label={`Copy request id ${id}`} onClick={() => void navigator.clipboard.writeText(id).then(() => setCopied(true), () => setCopied(false))}>{copied ? "Copied" : "Copy"}</button>
    </span>
  );
}

/**
 * Retry and cancel for one job, as the server allows them for the caller. Retry queues a new
 * job (the failed one stays as it was); cancelling asks first and says where the job stops.
 * `onDone` gets the new job's id after a retry.
 */
export function useJobControl(onDone: (retriedAs?: string) => void) {
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<{ id: string; message: string } | null>(null);
  const [toCancel, setToCancel] = useState<Job | null>(null);

  const retry = async (j: Job) => {
    setBusy(j.id); setError(null);
    try {
      const res = await client.request("retry_job", { params: { id: j.id }, idempotencyKey: idempotencyKey() });
      onDone(res.jobId);
    } catch (e) {
      setError({ id: j.id, message: messageOf(e) });
    } finally {
      setBusy(null);
    }
  };

  const cancel = async (j: Job) => {
    setToCancel(null); setBusy(j.id); setError(null);
    try {
      await client.request("cancel_job", { params: { id: j.id } });
      onDone();
    } catch (e) {
      setError({ id: j.id, message: messageOf(e) });
    } finally {
      setBusy(null);
    }
  };

  const dialog = (
    <ConfirmDialog
      open={!!toCancel}
      title={`Cancel ${toCancel ? kindLabel(toCancel.kind).toLowerCase() : "job"}?`}
      body={toCancel && (
        <p>
          {toCancel.status === "pending"
            ? "It has not started, so nothing in Incus changes."
            : "It is running: it stops at its next safe point, never in the middle of a change Incus is making. What it already did stays done."}
        </p>
      )}
      action="Cancel job"
      dismiss="Keep it"
      onCancel={() => setToCancel(null)}
      onConfirm={() => { if (toCancel) void cancel(toCancel); }}
    />
  );

  return { busy, error, retry, askCancel: setToCancel, dialog };
}
