import type { AppDetailView, OperationDto } from "../../../features/applications/model";
import { timeAgo } from "../common";

const STATE_TONE: Record<string, string> = { succeeded: "badge-success", failed: "badge-error", running: "badge-info", pending: "badge-ghost", cancelled: "badge-neutral" };

export function OperationList({ operations }: { operations: OperationDto[] }) {
  if (operations.length === 0) return <p className="rounded-box border border-dashed border-base-300 px-4 py-6 text-center text-xs text-base-content/60" data-testid="no-operations">No start, stop or restart has been run yet.</p>;
  return (
    <ul className="flex flex-col gap-2" data-testid="operations">
      {operations.map((o) => (
        <li key={o.id} className="rounded-box border border-base-300 bg-base-100 p-3" data-testid="operation" data-state={o.state}>
          <details open={o.state === "failed"}>
            <summary className="flex cursor-pointer flex-wrap items-center gap-2 text-sm">
              <span className="font-medium capitalize">{o.kind}</span>
              <span className={`badge badge-soft badge-sm ${STATE_TONE[o.state] ?? "badge-ghost"}`}>{o.state}</span>
              <span className="text-xs text-base-content/60">{o.stepsDone}/{o.stepsTotal} steps · {timeAgo(o.createdAt)} · by {o.requestedBy}</span>
            </summary>
            {o.error && <p role="alert" className="mt-2 rounded-box border border-error/40 bg-error/5 px-3 py-2 text-xs text-error" data-testid="operation-error">{o.error}</p>}
            <ol className="mt-2 flex flex-col gap-2 text-xs">
              {o.phases.map((p, i) => (
                <li key={i}>
                  <div className="font-medium capitalize">Phase {i + 1}: {p.action} <span className="font-normal text-base-content/60">({p.state})</span></div>
                  <ul className="ml-3 mt-1 flex flex-col gap-0.5">
                    {p.steps.map((s) => (
                      <li key={s.instance} className="flex flex-wrap gap-2"><span className="font-mono">{s.instance}</span><span className="text-base-content/60">{s.state}</span>{s.error && <span className="text-error">{s.error}</span>}</li>
                    ))}
                  </ul>
                </li>
              ))}
            </ol>
          </details>
        </li>
      ))}
    </ul>
  );
}

export function Deployments({ detail, onDeploy, canDeploy, deployReason }: { detail: AppDetailView; onDeploy: (version?: number) => void; canDeploy: boolean; deployReason: string | null }) {
  const published = detail.versions.filter((v) => v.published);
  const deployed = detail.application.deployment.deployedVersion;
  return (
    <div className="flex flex-col gap-5" data-testid="tab-deployments">
      <section className="flex flex-col gap-2">
        <div className="flex items-center justify-between">
          <h2 className="text-sm font-semibold">Versions to deploy</h2>
          <button type="button" className="btn btn-sm btn-primary" disabled={!canDeploy || published.length === 0} title={deployReason ?? undefined} onClick={() => onDeploy()} data-testid="deploy-version">Deploy version…</button>
        </div>
        {published.length === 0 ? (
          <p className="rounded-box border border-dashed border-base-300 px-4 py-6 text-center text-xs text-base-content/60" data-testid="deploy-nothing-published">Nothing is published, so there is nothing to deploy. Drafts cannot be started or deployed; publish a version from the Designer first.</p>
        ) : (
          <ul className="flex flex-col gap-1.5">
            {published.map((v) => (
              <li key={v.revision} className="flex flex-wrap items-center gap-2 rounded-box border border-base-300 px-3 py-2 text-sm" data-testid="deployable-version">
                <span className="font-medium">Version {v.revision}</span>{v.label && <span className="text-base-content/70">{v.label}</span>}
                {deployed === v.revision && <span className="badge badge-soft badge-success badge-sm">Deployed</span>}
                <span className="text-xs text-base-content/60">{timeAgo(v.createdAt)}</span>
                <button type="button" className="btn btn-xs ml-auto" disabled={!canDeploy} title={deployReason ?? undefined} onClick={() => onDeploy(v.revision)} data-testid={`deploy-v${v.revision}`}>Deploy</button>
              </li>
            ))}
          </ul>
        )}
        {deployReason && <p className="text-xs text-base-content/60" data-testid="deploy-reason">{deployReason}</p>}
      </section>
      <section className="flex flex-col gap-2">
        <h2 className="text-sm font-semibold">Operations</h2>
        <OperationList operations={detail.operations} />
      </section>
    </div>
  );
}
