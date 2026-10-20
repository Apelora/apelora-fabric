import type { AppDetailView } from "../../../features/applications/model";
import { timeAgo } from "../common";

export function Versions({ detail, onPublish, onDeploy, canDeploy, deployReason }: { detail: AppDetailView; onPublish: () => void; onDeploy: (v: number) => void; canDeploy: boolean; deployReason: string | null }) {
  const a = detail.application;
  return (
    <div className="flex flex-col gap-3" data-testid="tab-versions">
      <div className="flex items-center justify-between">
        <div>
          <h2 className="text-sm font-semibold">Versions</h2>
          <p className="text-xs text-base-content/60">A published version never changes. The draft ({a.design.draftVersion != null ? `version ${a.design.draftVersion}` : "none"}) is where you keep editing.</p>
        </div>
        <button type="button" className="btn btn-sm btn-primary" onClick={onPublish} disabled={a.design.draftVersion == null} data-testid="versions-publish">Publish draft…</button>
      </div>
      {detail.versions.length === 0 ? (
        <p className="rounded-box border border-dashed border-base-300 px-4 py-8 text-center text-xs text-base-content/60" data-testid="no-versions">No version has been published yet.</p>
      ) : (
        <div className="overflow-x-auto rounded-box border border-base-300">
          <table className="table table-sm" data-testid="version-table">
            <thead><tr><th>Version</th><th>Label</th><th>From draft</th><th>Published</th><th>By</th><th className="text-right">Deploy</th></tr></thead>
            <tbody>{detail.versions.map((v) => (
              <tr key={v.revision} data-testid="version-row">
                <td className="font-medium">{v.revision}{a.deployment.deployedVersion === v.revision && <span className="badge badge-soft badge-success badge-sm ml-2">Deployed</span>}</td>
                <td>{v.label ?? "—"}</td><td>{v.draftVersion ?? "—"}</td><td title={v.createdAt}>{timeAgo(v.createdAt)}</td><td>{v.createdBy}</td>
                <td className="text-right"><button type="button" className="btn btn-xs" disabled={!canDeploy} title={deployReason ?? undefined} onClick={() => onDeploy(v.revision)} data-testid={`versions-deploy-${v.revision}`}>Deploy</button></td>
              </tr>
            ))}</tbody>
          </table>
        </div>
      )}
      {deployReason && <p className="text-xs text-base-content/60">{deployReason}</p>}
    </div>
  );
}
