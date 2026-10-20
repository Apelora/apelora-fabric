import { useId, useState } from "react";

import type { AssignmentDto, Scope, WhoAmI } from "@apelora/api-client";

import { Badge, ConfirmDialog, ErrorPanel, Notice, Section, SectionHeader, TableFrame } from "../../components/ui";
import { client, useLoad } from "../../lib/api";
import { ENV_LABEL, LEVEL_LABEL, LEVEL_ORDER, PRINCIPAL_LABEL, UUID, actorLabel, dataOf, fmtTime, holds, scopeKey, useDirectory, useRun, type Names } from "../../lib/identity";
import { AssignmentDialog, type AssignPreset } from "./AssignmentDialog";
import { Picker } from "./Picker";

type Mode = "all" | Scope["type"];
const MODES: { value: Mode; label: string }[] = [
  { value: "all", label: "Everything I can see" },
  { value: "platform", label: "Platform" },
  { value: "organization", label: "One organization" },
  { value: "project", label: "One project" },
  { value: "application", label: "One application" },
];

const conditions = (a: AssignmentDto): string => {
  const parts: string[] = [];
  if (a.conditions.requireMfa) parts.push("multi-factor sign-in required");
  if (a.conditions.environments?.length) parts.push(`only in ${a.conditions.environments.map((k) => ENV_LABEL[k].toLowerCase()).join(", ")}`);
  return parts.join("; ") || "None";
};

function AssignmentRows({ items, names, canRevoke, onRevoke }: { items: AssignmentDto[]; names: Names; canRevoke: boolean; onRevoke: (a: AssignmentDto) => void }) {
  return (
    <TableFrame fixed>
      <thead><tr><th className="w-[26%]">Who</th><th className="w-[24%]">Role</th><th className="w-[24%] max-md:hidden">Conditions</th><th className="w-[12%] max-sm:hidden">Expires</th><th className="w-[18%] max-lg:hidden">Added by</th><th className="w-24"><span className="sr-only">Actions</span></th></tr></thead>
      <tbody>
        {items.map((a) => {
          const who = names.principal(a.principal);
          return (
            <tr key={a.id} data-testid="assignment-row">
              <td className="font-medium">{who}<span className="block text-xs font-normal text-base-content/60">{PRINCIPAL_LABEL[a.principal.type]}</span></td>
              <td>{a.roleName || "Unknown role"} {a.kind === "emergency" && <Badge tone="warn">Emergency</Badge>}</td>
              <td className="max-md:hidden">{conditions(a)}</td>
              <td className="whitespace-nowrap max-sm:hidden">{a.expiresAt ? fmtTime(a.expiresAt) : "Never"}</td>
              <td className="text-base-content/70 max-lg:hidden"><span title={a.createdBy}>{actorLabel(a.createdBy, names)}</span>{a.reason && <span className="line-clamp-2 block text-xs" title={a.reason}>{a.reason}</span>}</td>
              <td className="text-right">
                {canRevoke && <button type="button" className="btn btn-xs btn-ghost text-error" aria-label={`Revoke ${a.roleName} from ${who}`} onClick={() => onRevoke(a)}>Revoke…</button>}
              </td>
            </tr>
          );
        })}
      </tbody>
    </TableFrame>
  );
}

export function MembershipsView({ me, onAuth }: { me: WhoAmI | null; onAuth: (m: string) => void }) {
  const [assignments, reload] = useLoad<AssignmentDto[]>(() => client.request("assignments", { query: { includeRevoked: false } }).then((r) => r.items), onAuth);
  const dir = useDirectory(me, onAuth, { orgs: true, projects: true, groups: true, users: true, services: true });
  const [mode, setMode] = useState<Mode>("all");
  const [orgId, setOrgId] = useState("");
  const [projectId, setProjectId] = useState("");
  const [appId, setAppId] = useState("");
  const [assign, setAssign] = useState<AssignPreset | null>(null);
  const [toRevoke, setToRevoke] = useState<AssignmentDto | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const { error, run } = useRun(onAuth);
  const modeId = useId();
  const appInput = useId();
  const canAssign = holds(me, "role.assign");

  const selected: Scope | null =
    mode === "platform" ? { type: "platform" }
      : mode === "organization" ? (orgId ? { type: "organization", id: orgId } : null)
        : mode === "project" ? (projectId ? { type: "project", id: projectId } : null)
          : mode === "application" ? (UUID.test(appId.trim()) ? { type: "application", id: appId.trim().toLowerCase() } : null)
            : null;
  const project = selected?.type === "project" ? dataOf(dir.projects).find((p) => p.id === selected.id) : undefined;
  // A role given higher up applies below it, so those assignments count for the chosen scope too.
  const relevant = new Set<string>();
  if (selected) {
    relevant.add(scopeKey(selected));
    if (selected.type !== "platform") relevant.add("platform");
    if (project) relevant.add(`organization:${project.organizationId}`);
  }

  const revoke = (a: AssignmentDto) => {
    setNotice(null);
    void run(async () => {
      await client.request("revoke_assignment", { params: { assignment: a.id } });
      setNotice(`${a.roleName} was revoked from ${dir.names.principal(a.principal)} at ${dir.names.scope(a.scope)}. They lose it on their next request.`);
      reload();
    });
  };

  return (
    <>
      <Section title="Project memberships" level={1} state={assignments} testId="memberships" onRetry={reload}>
        {(all) => {
          const shown = mode === "all" ? all : selected ? all.filter((a) => relevant.has(scopeKey(a.scope))) : [];
          const groups = new Map<string, { scope: Scope; items: AssignmentDto[] }>();
          for (const a of shown) {
            const k = scopeKey(a.scope);
            const g = groups.get(k) ?? { scope: a.scope, items: [] };
            g.items.push(a);
            groups.set(k, g);
          }
          const ordered = [...groups.entries()].sort(([, a], [, b]) => LEVEL_ORDER.indexOf(a.scope.type) - LEVEL_ORDER.indexOf(b.scope.type) || dir.names.scope(a.scope).localeCompare(dir.names.scope(b.scope)));
          return (
            <>
              <SectionHeader title="Project memberships" level={1} count={shown.length}>
                {canAssign && <button type="button" className="btn btn-sm btn-primary" onClick={() => setAssign(selected ? { scope: selected } : {})} data-testid="assign-open">Assign a role…</button>}
              </SectionHeader>
              <p className="text-sm text-base-content/70">Who holds which role where. A role given at a higher level applies to everything below it, so those entries are shown with the narrower scope.</p>

              <div className="grid items-start gap-3 rounded-box border border-base-300 p-4 sm:grid-cols-2" data-testid="membership-scope">
                <div className="flex flex-col gap-1 text-sm">
                  <label htmlFor={modeId}>Show</label>
                  <select id={modeId} className="select select-sm w-full" value={mode} onChange={(e) => setMode(e.target.value as Mode)} data-testid="membership-mode">
                    {MODES.map((m) => <option key={m.value} value={m.value}>{m.label}</option>)}
                  </select>
                </div>
                {mode === "organization" && <Picker label="Organization" data={dir.orgs} value={orgId} onChange={setOrgId} id={(o) => o.id} text={(o) => o.name} testId="membership-org" offMessage="You may not list organizations." emptyMessage="There are no organizations yet." />}
                {mode === "project" && <Picker label="Project" data={dir.projects} value={projectId} onChange={setProjectId} id={(p) => p.id} text={(p) => `${p.organizationSlug} / ${p.name}`} testId="membership-project" offMessage="You may not list projects." emptyMessage="There are no projects yet." />}
                {mode === "application" && (
                  <div className="flex flex-col gap-1 text-sm">
                    <label htmlFor={appInput}>Application id</label>
                    <input id={appInput} className="input input-sm w-full font-mono" value={appId} onChange={(e) => setAppId(e.target.value)} placeholder="00000000-0000-0000-0000-000000000000" autoComplete="off" spellCheck={false} data-testid="membership-app" />
                    <span className="text-xs text-base-content/60">The API does not list application ids yet, so paste the id of the application.</span>
                  </div>
                )}
              </div>

              {notice && <Notice tone="ok" testId="memberships-notice">{notice}</Notice>}
              {error && <ErrorPanel message={error} testId="memberships-error" />}

              {mode !== "all" && !selected ? (
                <p className="text-sm text-base-content/70" data-testid="memberships-choose">Choose {mode === "organization" ? "an organization" : mode === "project" ? "a project" : "an application id"} to see who holds roles there.</p>
              ) : ordered.length === 0 ? (
                <p className="text-sm text-base-content/70" data-testid="memberships-empty">{mode === "all" ? "No role assignments in the scopes you can see." : "No one holds a role here or above it."}</p>
              ) : (
                ordered.map(([k, g]) => (
                  <section key={k} className="flex flex-col gap-2" aria-label={dir.names.scope(g.scope)} data-testid="membership-group">
                    <h2 className="flex flex-wrap items-center gap-2 text-sm font-semibold">
                      {dir.names.scope(g.scope)}
                      <span className="font-normal text-base-content/60">({LEVEL_LABEL[g.scope.type]})</span>
                      {selected && k !== scopeKey(selected) && <Badge tone="info">Inherited</Badge>}
                    </h2>
                    <AssignmentRows items={g.items} names={dir.names} canRevoke={canAssign} onRevoke={setToRevoke} />
                  </section>
                ))
              )}
            </>
          );
        }}
      </Section>

      <AssignmentDialog open={!!assign} preset={assign ?? {}} me={me} onAuth={onAuth} onClose={() => setAssign(null)} onSaved={(m) => { setAssign(null); setNotice(m); reload(); }} />
      <ConfirmDialog
        open={!!toRevoke}
        title={toRevoke ? `Revoke ${toRevoke.roleName} from ${dir.names.principal(toRevoke.principal)}?` : "Revoke assignment?"}
        body={<p>{toRevoke ? `They lose this role at ${dir.names.scope(toRevoke.scope)} on their next request. The server refuses to revoke the last platform super admin.` : ""}</p>}
        action="Revoke assignment"
        onCancel={() => setToRevoke(null)}
        onConfirm={() => { const a = toRevoke; setToRevoke(null); if (a) revoke(a); }}
      />
    </>
  );
}
