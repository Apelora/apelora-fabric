import { healthLine, type AppDetailView } from "../../../features/applications/model";
import { onLinkClick, paths } from "../../../features/router";
import { DesignBadge, RuntimeBadge, timeAgo } from "../common";
import { OperationList } from "./Deployments";

function Card({ title, children, testId }: { title: string; children: React.ReactNode; testId: string }) {
  return (
    <section className="flex flex-col gap-2 rounded-box border border-base-300 bg-base-100 p-4" data-testid={testId}>
      <h2 className="text-xs font-semibold uppercase tracking-wide text-base-content/60">{title}</h2>
      {children}
    </section>
  );
}

export function Overview({ detail, onPublish }: { detail: AppDetailView; onPublish: () => void }) {
  const a = detail.application;
  const d = a.deployment;
  return (
    <div className="flex flex-col gap-4" data-testid="tab-overview">
      <div className="grid gap-3 md:grid-cols-3">
        <Card title="Status" testId="card-status">
          <div className="flex items-center gap-2"><RuntimeBadge app={a} /><span className="text-sm" data-testid="overview-health">{healthLine(a)}</span></div>
          <p className="text-xs text-base-content/60">{d.source === "none" ? "This browser cannot observe a cluster, so the runtime state is unknown." : `Observed ${timeAgo(d.observedAt)}${d.source === "mock" ? " from mock data, not a real cluster" : " from Incus"}.`}</p>
          {d.stale && d.source !== "none" && <p role="status" className="text-xs text-warning" data-testid="overview-stale">Could not refresh; this is not shown as healthy.</p>}
          {d.deployedVersion != null && <p className="text-xs">Deployed from version {d.deployedVersion}</p>}
        </Card>
        <Card title="Design" testId="card-design">
          <div className="flex items-center gap-2"><DesignBadge app={a} /></div>
          <dl className="grid grid-cols-2 gap-1 text-xs">
            <dt className="text-base-content/60">Draft</dt><dd>{a.design.draftVersion != null ? `version ${a.design.draftVersion}` : "none"}</dd>
            <dt className="text-base-content/60">Published</dt><dd>{a.design.publishedVersion != null ? `version ${a.design.publishedVersion}` : "nothing yet"}</dd>
            <dt className="text-base-content/60">Problems</dt><dd>{a.design.validationErrors}</dd>
          </dl>
          <div className="mt-auto flex gap-2">
            <a href={paths.application(a.name, "designer")} onClick={onLinkClick} className="btn btn-xs btn-primary" data-testid="overview-open-designer">Open designer</a>
            <button type="button" className="btn btn-xs" onClick={onPublish} data-testid="overview-publish" disabled={a.design.draftVersion == null}>Publish…</button>
          </div>
        </Card>
        <Card title="Where it runs" testId="card-where">
          <dl className="grid grid-cols-2 gap-1 text-xs">
            <dt className="text-base-content/60">Organisation</dt><dd>{a.project.organizationSlug}</dd>
            <dt className="text-base-content/60">Project</dt><dd>{a.project.name}</dd>
            <dt className="text-base-content/60">Environment</dt><dd>{a.environment ? `${a.environment.name} (${a.environment.kind})` : "—"}</dd>
            <dt className="text-base-content/60">Incus project</dt><dd>{a.environment?.incusProject ?? "—"}</dd>
          </dl>
        </Card>
      </div>
      <section className="flex flex-col gap-2" aria-label="Modules">
        <h2 className="text-sm font-semibold">Modules</h2>
        {detail.modules.length === 0 ? (
          <p className="rounded-box border border-dashed border-base-300 px-4 py-6 text-center text-xs text-base-content/60" data-testid="overview-no-modules">{d.status === "not-deployed" ? "Nothing is deployed. Publish a version and deploy it to see its modules here." : "No module instances are known."}</p>
        ) : (
          <div className="overflow-x-auto rounded-box border border-base-300">
            <table className="table table-sm" data-testid="overview-modules">
              <thead><tr><th>Module</th><th>Availability</th><th>Replicas</th><th>Healthy</th></tr></thead>
              <tbody>{detail.modules.map((x) => (
                <tr key={x.module}><td className="font-medium">{x.module}</td><td>{x.availability}</td><td>{x.replicas}</td><td>{x.instances.filter((i) => !i.unhealthy && i.status.toLowerCase() === "running").length}/{x.instances.length}</td></tr>
              ))}</tbody>
            </table>
          </div>
        )}
      </section>
      <section className="flex flex-col gap-2" aria-label="Recent operations">
        <h2 className="text-sm font-semibold">Recent operations</h2>
        <OperationList operations={detail.operations.slice(0, 5)} />
      </section>
    </div>
  );
}
