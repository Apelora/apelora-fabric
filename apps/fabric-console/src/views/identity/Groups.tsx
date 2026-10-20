import { useId, useState } from "react";

import type { GroupDto, UserDto, WhoAmI } from "@apelora/api-client";

import { Badge, ConfirmDialog, ErrorPanel, FormDialog, Notice, Section, SectionHeader, TableFrame } from "../../components/ui";
import { client, useLoad } from "../../lib/api";
import { holds, holdsPlatform, personName, useDirectory, useRun, type Directory } from "../../lib/identity";
import { AssignmentDialog, type AssignPreset } from "./AssignmentDialog";
import { PersonLookup } from "./PersonLookup";

function CreateGroupDialog({ open, me, dir, onAuth, onClose, onCreated }: { open: boolean; me: WhoAmI | null; dir: Directory; onAuth: (m: string) => void; onClose: () => void; onCreated: (g: GroupDto) => void }) {
  return (
    <FormDialog open={open} title="Create a group" onClose={onClose} testId="group-dialog">
      {open && <CreateGroupForm me={me} dir={dir} onAuth={onAuth} onClose={onClose} onCreated={onCreated} />}
    </FormDialog>
  );
}

function CreateGroupForm({ me, dir, onAuth, onClose, onCreated }: { me: WhoAmI | null; dir: Directory; onAuth: (m: string) => void; onClose: () => void; onCreated: (g: GroupDto) => void }) {
  const platform = holdsPlatform(me, "group.manage");
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [orgId, setOrgId] = useState(platform ? "" : "-");
  const [external, setExternal] = useState("");
  const { busy, error, run } = useRun(onAuth);
  const nameId = useId();
  const descId = useId();
  const orgSel = useId();
  const extId = useId();
  const orgs = dir.orgs.kind === "ok" ? dir.orgs.data : [];
  const nameOk = name.trim().length >= 1 && name.trim().length <= 80;
  const orgChosen = orgId !== "-";
  const submit = () => {
    void run(async () => {
      const g = await client.request("create_group", { body: { name: name.trim(), description: description.trim(), organizationId: orgId && orgChosen ? orgId : null, externalName: platform && external.trim() ? external.trim() : null } });
      onCreated(g);
    });
  };
  return (
    <form className="flex flex-col gap-3" onSubmit={(e) => { e.preventDefault(); if (nameOk && orgChosen) submit(); }}>
      <div className="flex flex-col gap-1 text-sm">
        <label htmlFor={nameId}>Name</label>
        <input id={nameId} className="input input-sm w-full" value={name} onChange={(e) => setName(e.target.value)} maxLength={80} autoComplete="off" data-testid="group-name" />
      </div>
      <div className="flex flex-col gap-1 text-sm">
        <label htmlFor={descId}>Description (optional)</label>
        <input id={descId} className="input input-sm w-full" value={description} onChange={(e) => setDescription(e.target.value)} maxLength={200} autoComplete="off" data-testid="group-description" />
      </div>
      <div className="flex flex-col gap-1 text-sm">
        <label htmlFor={orgSel}>Belongs to</label>
        <select id={orgSel} className="select select-sm w-full" value={orgId} onChange={(e) => setOrgId(e.target.value)} data-testid="group-org">
          {!platform && <option value="-">Choose an organization…</option>}
          {platform && <option value="">Platform (no organization)</option>}
          {orgs.map((o) => <option key={o.id} value={o.id}>{o.name}</option>)}
        </select>
        <span className="text-xs text-base-content/60">
          {dir.orgs.kind === "off" ? "You may not list organizations." : platform ? "A platform group can be given roles anywhere; an organization's group only inside that organization." : "An organization's group can only be given roles inside that organization."}
        </span>
      </div>
      {platform ? (
        <div className="flex flex-col gap-1 text-sm">
          <label htmlFor={extId}>Identity provider group name (optional)</label>
          <input id={extId} className="input input-sm w-full font-mono" value={external} onChange={(e) => setExternal(e.target.value)} autoComplete="off" spellCheck={false} data-testid="group-external" />
          <span className="text-xs text-base-content/60">The group&apos;s name in the identity provider&apos;s tokens. Its members then come from the provider and cannot be changed here.</span>
        </div>
      ) : (
        <p className="text-xs text-base-content/60">Mapping a group to an identity provider group is for platform administrators.</p>
      )}
      {error && <ErrorPanel message={error} testId="group-error" />}
      <div className="modal-action mt-0">
        <button type="button" className="btn btn-sm btn-ghost" onClick={onClose}>Cancel</button>
        <button type="submit" className="btn btn-sm btn-primary" disabled={busy || !nameOk || !orgChosen} data-testid="group-submit">{busy ? "Creating…" : "Create group"}</button>
      </div>
    </form>
  );
}

function GroupMembers({ group, me, orgName, onAuth }: { group: GroupDto; me: WhoAmI | null; orgName: string; onAuth: (m: string) => void }) {
  const [members, reload] = useLoad<UserDto[]>(() => client.request("group_members", { params: { group: group.id } }).then((r) => r.items), onAuth);
  const [toRemove, setToRemove] = useState<UserDto | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const { error, run } = useRun(onAuth);
  const managed = !!group.externalName;
  const canManage = holds(me, "group.manage");
  const add = (u: UserDto) => {
    setNotice(null);
    void run(async () => {
      await client.request("add_group_member", { params: { group: group.id, user: u.id } });
      setNotice(`${personName(u)} is now a member of ${group.name}.`);
      reload();
    });
  };
  const remove = (u: UserDto) => {
    setNotice(null);
    void run(async () => {
      await client.request("remove_group_member", { params: { group: group.id, user: u.id } });
      setNotice(`${personName(u)} was removed from ${group.name}.`);
      reload();
    });
  };
  return (
    <>
      <Section title={`Members of ${group.name}`} state={members} testId="group-members" onRetry={reload}>
        {(list) => (
          <>
            <SectionHeader title={`Members of ${group.name}`} count={list.length} />
            <p className="text-sm text-base-content/70">{group.organizationId ? `Group of ${orgName}.` : "Platform group."} {group.description}</p>
            {managed && (
              <Notice tone="info" testId="group-managed-note">
                Members of this group come from the identity provider (group name <span className="font-mono">{group.externalName}</span>), so they are listed here but cannot be added or removed here. Change them in the identity provider; they appear after their next sign-in.
              </Notice>
            )}
            {notice && <Notice tone="ok" testId="group-notice">{notice}</Notice>}
            {error && <ErrorPanel message={error} testId="group-member-error" />}
            {!managed && canManage && (
              <div className="rounded-box border border-base-300 p-4">
                <PersonLookup onAuth={onAuth} label="Add a person to this group" actionLabel="Add to group" testId="group-add" onPick={add} />
              </div>
            )}
            {list.length === 0 ? <p className="text-sm text-base-content/70" data-testid="group-members-empty">This group has no members yet.</p> : (
              <TableFrame>
                <thead><tr><th>Name</th><th className="max-sm:hidden">E-mail</th><th>Status</th><th><span className="sr-only">Actions</span></th></tr></thead>
                <tbody>
                  {list.map((u) => (
                    <tr key={u.id} data-testid="group-member-row">
                      <td className="font-medium">{personName(u)}</td>
                      <td className="max-sm:hidden">{u.email ?? "—"}</td>
                      <td>{u.disabled ? <Badge tone="error">Disabled</Badge> : <Badge tone="ok">Active</Badge>}</td>
                      <td className="text-right">
                        {!managed && canManage && <button type="button" className="btn btn-xs btn-ghost text-error" aria-label={`Remove ${personName(u)} from ${group.name}`} onClick={() => setToRemove(u)}>Remove…</button>}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </TableFrame>
            )}
          </>
        )}
      </Section>
      <ConfirmDialog
        open={!!toRemove}
        title={`Remove ${toRemove ? personName(toRemove) : ""} from ${group.name}?`}
        body={<p>Any access they have through this group ends on their next request.</p>}
        action="Remove from group"
        onCancel={() => setToRemove(null)}
        onConfirm={() => { const u = toRemove; setToRemove(null); if (u) remove(u); }}
      />
    </>
  );
}

export function GroupsView({ me, onAuth }: { me: WhoAmI | null; onAuth: (m: string) => void }) {
  const [groups, reload] = useLoad<GroupDto[]>(() => client.get("groups").then((r) => r.items), onAuth);
  const dir = useDirectory(me, onAuth, { orgs: true });
  const [creating, setCreating] = useState(false);
  const [selected, setSelected] = useState<string | null>(null);
  const [assign, setAssign] = useState<AssignPreset | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const canAssign = holds(me, "role.assign");
  const picked = groups.kind === "ok" ? groups.data.find((g) => g.id === selected) : undefined;
  return (
    <>
      <Section title="Groups" level={1} state={groups} testId="groups" onRetry={reload}>
        {(list) => (
          <>
            <SectionHeader title="Groups" level={1} count={list.length}>
              {holds(me, "group.manage") && <button type="button" className="btn btn-sm btn-primary" onClick={() => setCreating(true)} data-testid="group-open">Create group</button>}
            </SectionHeader>
            <p className="text-sm text-base-content/70">A group gets a role once and everyone in it holds it. Organization groups work only inside their organization.</p>
            {notice && <Notice tone="ok" testId="groups-notice">{notice}</Notice>}
            {list.length === 0 ? <p className="text-sm text-base-content/70" data-testid="groups-empty">No groups yet.</p> : (
              <TableFrame>
                <thead><tr><th>Group</th><th>Belongs to</th><th className="max-sm:hidden">Members come from</th><th><span className="sr-only">Actions</span></th></tr></thead>
                <tbody>
                  {list.map((g) => (
                    <tr key={g.id} data-testid="group-row" className={selected === g.id ? "bg-base-200" : undefined}>
                      <td className="font-medium">{g.name}{g.description && <span className="block text-xs font-normal text-base-content/60">{g.description}</span>}</td>
                      <td>{g.organizationId ? dir.names.org(g.organizationId) : "Platform"}</td>
                      <td className="max-sm:hidden">{g.externalName ? <Badge tone="info" title={`Identity provider group ${g.externalName}`}>Identity provider</Badge> : <Badge>Managed here</Badge>}</td>
                      <td className="whitespace-nowrap text-right">
                        <button type="button" className="btn btn-xs btn-ghost" aria-pressed={selected === g.id} aria-label={`Members of ${g.name}`} onClick={() => setSelected(selected === g.id ? null : g.id)}>Members</button>
                        {canAssign && <button type="button" className="btn btn-xs btn-ghost" aria-label={`Assign a role to ${g.name}`} onClick={() => setAssign({ principal: { ref: { type: "group", id: g.id }, label: g.name } })}>Assign role…</button>}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </TableFrame>
            )}
          </>
        )}
      </Section>

      {picked && <GroupMembers key={picked.id} group={picked} me={me} orgName={picked.organizationId ? dir.names.org(picked.organizationId) : ""} onAuth={onAuth} />}

      <CreateGroupDialog open={creating} me={me} dir={dir} onAuth={onAuth} onClose={() => setCreating(false)} onCreated={(g) => { setCreating(false); setNotice(`Group ${g.name} created.`); setSelected(g.id); reload(); }} />
      <AssignmentDialog open={!!assign} preset={assign ?? {}} me={me} onAuth={onAuth} onClose={() => setAssign(null)} onSaved={(m) => { setAssign(null); setNotice(m); }} />
    </>
  );
}
