import { useId, useState } from "react";

import type { Class, PermissionDto, RoleDto, WhoAmI } from "@apelora/api-client";

import { Badge, ConfirmDialog, ErrorPanel, Notice, Section, SectionHeader, TableFrame } from "../../components/ui";
import { client, useLoad } from "../../lib/api";
import { CLASS_LABEL, CLASS_ORDER, LEVEL_LABEL, holdsPlatform, useRun } from "../../lib/identity";
import { CreateRoleDialog } from "./CreateRoleDialog";

type Data = { roles: RoleDto[]; permissions: PermissionDto[] };

function PermissionLine({ id, catalogue }: { id: string; catalogue: PermissionDto[] }) {
  const def = catalogue.find((p) => p.id === id);
  return (
    <li className="flex flex-wrap items-baseline gap-x-2 gap-y-0.5 text-sm" data-testid="role-permission">
      <span className="font-mono text-xs">{id}</span>
      {def && <span className="text-base-content/60">{def.description}</span>}
      {def?.needsApproval && <Badge tone="warn">Needs approval</Badge>}
      {def && <span className="text-xs text-base-content/60">narrowest scope: {LEVEL_LABEL[def.floor]}</span>}
    </li>
  );
}

function RoleCard({ role, catalogue, onDelete }: { role: RoleDto; catalogue: PermissionDto[]; onDelete?: (r: RoleDto) => void }) {
  const byClass = new Map<Class | "unknown", string[]>();
  for (const id of role.permissions) {
    const c = catalogue.find((p) => p.id === id)?.class ?? "unknown";
    byClass.set(c, [...(byClass.get(c) ?? []), id]);
  }
  return (
    <article className="flex flex-col gap-2 rounded-box border border-base-300 p-4" aria-label={role.name} data-testid="role-card" data-role={role.key}>
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h3 className="flex flex-wrap items-center gap-2 font-semibold">{role.name} <Badge>{role.builtin ? "Predefined" : "Custom"}</Badge></h3>
        {onDelete && <button type="button" className="btn btn-xs btn-ghost text-error" aria-label={`Delete the role ${role.name}`} onClick={() => onDelete(role)}>Delete…</button>}
      </div>
      {role.description && <p className="text-sm text-base-content/70">{role.description}</p>}
      <p className="text-sm" data-testid="role-scopes">Can be assigned at: {role.scopes.map((l) => LEVEL_LABEL[l]).join(", ") || "nowhere"}.</p>
      <details className="text-sm">
        <summary className="cursor-pointer">{role.permissions.length} {role.permissions.length === 1 ? "permission" : "permissions"}</summary>
        <div className="mt-2 flex flex-col gap-3">
          {[...CLASS_ORDER, "unknown" as const].map((c) => {
            const ids = byClass.get(c);
            if (!ids) return null;
            return (
              <div key={c} className="flex flex-col gap-1">
                <h4 className="text-xs font-semibold uppercase tracking-wide text-base-content/60">{c === "unknown" ? "Not in the catalogue" : CLASS_LABEL[c]} ({ids.length})</h4>
                <ul className="flex flex-col gap-1">{ids.map((id) => <PermissionLine key={id} id={id} catalogue={catalogue} />)}</ul>
              </div>
            );
          })}
        </div>
      </details>
    </article>
  );
}

function Catalogue({ items }: { items: PermissionDto[] }) {
  const [cls, setCls] = useState<Class | "all">("all");
  const selectId = useId();
  const shown = items.filter((p) => cls === "all" || p.class === cls);
  return (
    <section className="flex flex-col gap-2" aria-label="Permission catalogue" data-testid="catalogue">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h2 className="font-semibold">Permission catalogue <span className="font-normal text-base-content/60">({shown.length})</span></h2>
        <div className="flex items-center gap-2 text-sm">
          <label htmlFor={selectId}>Class</label>
          <select id={selectId} className="select select-sm" value={cls} onChange={(e) => setCls(e.target.value as Class | "all")} data-testid="catalogue-class">
            <option value="all">All</option>
            {CLASS_ORDER.map((c) => <option key={c} value={c}>{CLASS_LABEL[c]}</option>)}
          </select>
        </div>
      </div>
      <p className="text-sm text-base-content/70">Every protected action names one permission. Only tenant permissions can be granted below the platform; the narrowest scope is the lowest level at which a grant takes effect.</p>
      <TableFrame>
        <thead><tr><th>Permission</th><th>Class</th><th className="max-sm:hidden">Narrowest scope</th><th>Approval</th><th className="max-md:hidden">What it allows</th></tr></thead>
        <tbody>
          {shown.map((p) => (
            <tr key={p.id} data-testid="catalogue-row">
              <td className="font-mono text-xs">{p.id}</td>
              <td>{CLASS_LABEL[p.class]}</td>
              <td className="max-sm:hidden">{LEVEL_LABEL[p.floor]}</td>
              <td>{p.needsApproval ? <Badge tone="warn">Needs approval</Badge> : <span className="text-base-content/60">No</span>}</td>
              <td className="max-md:hidden">{p.description}</td>
            </tr>
          ))}
        </tbody>
      </TableFrame>
    </section>
  );
}

export function RolesView({ me, onAuth }: { me: WhoAmI | null; onAuth: (m: string) => void }) {
  const [data, reload] = useLoad<Data>(async () => {
    const [roles, permissions] = await Promise.all([client.get("roles"), client.get("permissions")]);
    return { roles: roles.items, permissions: permissions.items };
  }, onAuth);
  const [creating, setCreating] = useState(false);
  const [toDelete, setToDelete] = useState<RoleDto | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const { error, run } = useRun(onAuth);
  const canManage = holdsPlatform(me, "role.manage");

  const remove = (r: RoleDto) => {
    if (r.reference.type !== "custom") return;
    const id = r.reference.id;
    setNotice(null);
    void run(async () => {
      await client.request("delete_role", { params: { role: id } });
      setNotice(`The role ${r.name} was deleted.`);
      reload();
    });
  };

  return (
    <>
      <Section title="Roles & permissions" level={1} state={data} testId="roles" onRetry={reload}>
        {({ roles, permissions }) => {
          const builtin = roles.filter((r) => r.builtin);
          const custom = roles.filter((r) => !r.builtin);
          return (
            <>
              <SectionHeader title="Roles & permissions" level={1}>
                {canManage && <button type="button" className="btn btn-sm btn-primary" onClick={() => setCreating(true)} data-testid="role-open">Create custom role</button>}
              </SectionHeader>
              <p className="text-sm text-base-content/70">A role is a set of permissions. Assign roles to people, groups or automation identities at a scope; permissions only count at or below the narrowest scope they allow.</p>
              {notice && <Notice tone="ok" testId="roles-notice">{notice}</Notice>}
              {error && <ErrorPanel message={error} testId="roles-error" />}

              <section className="flex flex-col gap-2" aria-label="Predefined roles" data-testid="builtin-roles">
                <h2 className="font-semibold">Predefined roles <span className="font-normal text-base-content/60">({builtin.length})</span></h2>
                <div className="grid gap-3 lg:grid-cols-2">{builtin.map((r) => <RoleCard key={r.key} role={r} catalogue={permissions} />)}</div>
              </section>

              <section className="flex flex-col gap-2" aria-label="Custom roles" data-testid="custom-roles">
                <h2 className="font-semibold">Custom roles <span className="font-normal text-base-content/60">({custom.length})</span></h2>
                {custom.length === 0 ? <p className="text-sm text-base-content/70" data-testid="custom-empty">No custom roles yet.{canManage ? "" : " Platform administrators can define them."}</p> : (
                  <div className="grid gap-3 lg:grid-cols-2">{custom.map((r) => <RoleCard key={r.key} role={r} catalogue={permissions} onDelete={canManage ? setToDelete : undefined} />)}</div>
                )}
              </section>

              <Catalogue items={permissions} />
              <CreateRoleDialog open={creating} catalogue={permissions} roles={roles} me={me} onAuth={onAuth} onClose={() => setCreating(false)} onCreated={(r) => { setCreating(false); setNotice(`The role ${r.name} was created. It can be assigned at: ${r.scopes.map((l) => LEVEL_LABEL[l]).join(", ")}.`); reload(); }} />
            </>
          );
        }}
      </Section>
      <ConfirmDialog
        open={!!toDelete}
        title={`Delete the role ${toDelete?.name ?? ""}?`}
        body={<p>A role that any assignment still uses cannot be deleted; the server says so if that is the case. Revoke or change those assignments first.</p>}
        action="Delete role"
        onCancel={() => setToDelete(null)}
        onConfirm={() => { const r = toDelete; setToDelete(null); if (r) remove(r); }}
      />
    </>
  );
}
