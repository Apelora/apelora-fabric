import type { AssignmentPreview, EffectivePermission, Grant } from "@apelora/api-client";

import { Badge, Notice, sentence } from "../../components/ui";
import { CLASS_LABEL, NIL_ID, type Names } from "../../lib/identity";

/** Where one part of a permission comes from: which role, at which scope, through which group. */
export function Origin({ grant, names }: { grant: Grant; names: Names }) {
  const fresh = grant.assignment === NIL_ID;
  return (
    <li className="flex flex-wrap items-center gap-x-2 gap-y-1" data-testid="origin">
      <span>
        <span className="font-medium">{grant.roleName || "Role"}</span> at {names.scope(grant.scope)}
        {grant.viaGroup ? <> via group <span className="font-medium">{names.group(grant.viaGroup)}</span></> : null}
      </span>
      {fresh && <Badge tone="info">This new assignment</Badge>}
      {grant.emergency && <Badge tone="warn">Emergency</Badge>}
    </li>
  );
}

function PermissionTable({ rows, added, names, label }: { rows: EffectivePermission[]; added?: Set<string>; names: Names; label: string }) {
  if (rows.length === 0) return <p className="text-sm text-base-content/70">Nothing.</p>;
  return (
    <div className="overflow-x-auto rounded-box border border-base-300">
      <table className="table table-sm" aria-label={label}>
        <thead><tr><th>Permission</th><th>Where it comes from</th></tr></thead>
        <tbody>
          {rows.map((p) => (
            <tr key={p.permission} data-testid="effective-permission" data-permission={p.permission}>
              <td className="align-top">
                <span className="font-mono text-xs">{p.permission}</span>
                {added?.has(p.permission) && <> <Badge tone="info">New</Badge></>}
                <span className="block text-xs text-base-content/60">{CLASS_LABEL[p.class]} · {p.description}</span>
              </td>
              <td className="align-top"><ul className="flex flex-col gap-1">{p.grants.map((g, i) => <Origin key={`${g.assignment}-${i}`} grant={g} names={names} />)}</ul></td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

/** What the signed-in administrator would be doing: may they save it, what changes, and why. */
export function PreviewPanel({ preview, names, who, scopeLabel, stale }: { preview: AssignmentPreview; names: Names; who: string; scopeLabel: string; stale: boolean }) {
  const added = new Set(preview.added);
  return (
    <section className={`flex flex-col gap-3 ${stale ? "opacity-60" : ""}`} aria-label="Preview of this assignment" aria-busy={stale} data-testid="assignment-preview">
      {preview.allowed
        ? <Notice tone="ok" testId="preview-allowed">You can save this assignment.</Notice>
        : <Notice tone="error" testId="preview-denied" title="You cannot save this assignment">{sentence(preview.error || "The server refused it.")}</Notice>}
      {preview.warnings.map((w) => <Notice key={w} tone="warn" testId="preview-warning">{sentence(w)}</Notice>)}

      <p className="text-sm" data-testid="preview-summary">
        {who} can do <strong>{preview.before.length}</strong> {preview.before.length === 1 ? "thing" : "things"} at {scopeLabel} now and would be able to do <strong>{preview.after.length}</strong> afterwards; this adds <strong>{preview.added.length}</strong>.
      </p>

      {preview.added.length > 0 && (
        <div className="flex flex-col gap-1" data-testid="preview-added">
          <h4 className="text-sm font-medium">Permissions this adds</h4>
          <ul className="flex flex-wrap gap-1">{preview.added.map((a) => <li key={a} className="badge badge-sm badge-outline font-mono">{a}</li>)}</ul>
        </div>
      )}
      {preview.inert.length > 0 && (
        <div className="flex flex-col gap-1" data-testid="preview-inert">
          <h4 className="text-sm font-medium">Permissions that do nothing at this scope</h4>
          <p className="text-xs text-base-content/60">The role contains them, but they only take effect at a narrower or different scope.</p>
          <ul className="flex flex-wrap gap-1">{preview.inert.map((a) => <li key={a} className="badge badge-sm badge-ghost font-mono">{a}</li>)}</ul>
        </div>
      )}

      <details open className="flex flex-col gap-2" data-testid="preview-after">
        <summary className="cursor-pointer text-sm font-medium">What they could do afterwards ({preview.after.length})</summary>
        <div className="mt-2"><PermissionTable rows={preview.after} added={added} names={names} label="Permissions afterwards" /></div>
      </details>
      <details className="flex flex-col gap-2" data-testid="preview-before">
        <summary className="cursor-pointer text-sm font-medium">What they can already do ({preview.before.length})</summary>
        <div className="mt-2"><PermissionTable rows={preview.before} names={names} label="Permissions now" /></div>
      </details>
    </section>
  );
}
