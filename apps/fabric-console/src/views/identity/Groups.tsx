import { useId, useState } from "react";

import type { GroupDto, GroupList, GroupSource, UserDto, WhoAmI } from "@apelora/api-client";

import { Badge, ConfirmDialog, ErrorPanel, FormDialog, Notice, Section, SectionHeader, TableFrame } from "../../components/ui";
import { client, useLoad, type Load } from "../../lib/api";
import { holds, holdsPlatform, personName, useDirectory, useRun, type Directory } from "../../lib/identity";
import { AssignmentDialog, type AssignPreset } from "./AssignmentDialog";
import { PersonLookup } from "./PersonLookup";

/** What a source is called on screen. */
function sourceName(sources: GroupSource[], id: string | null | undefined): string {
  const s = sources.find((x) => x.id === id);
  if (s) return s.name;
  return id?.startsWith("directory:") ? "a directory" : "an identity provider";
}

/** The source select and the external group's name, shared by creating a group and setting a source. */
function ExternalFields({ sources, source, setSource, external, setExternal, optional }: { sources: GroupSource[]; source: string; setSource: (v: string) => void; external: string; setExternal: (v: string) => void; optional: boolean }) {
  const srcId = useId();
  const extId = useId();
  const kind = sources.find((s) => s.id === source)?.kind;
  return (
    <>
      <div className="flex flex-col gap-1 text-sm">
        <label htmlFor={srcId}>Members come from</label>
        <select id={srcId} className="select select-sm w-full" value={source} onChange={(e) => setSource(e.target.value)} data-testid="group-source">
          {optional ? <option value="">This group (people added here)</option> : <option value="">Choose a source…</option>}
          {sources.map((s) => <option key={s.id} value={s.id}>{s.kind === "directory" ? `Directory: ${s.name}` : `Identity provider: ${s.name}`}</option>)}
        </select>
        <span className="text-xs text-base-content/60">{sources.length === 0 ? "No identity provider or directory is set up, so members are added here." : "Only people who sign in through this source are counted as members."}</span>
      </div>
      {source && (
        <div className="flex flex-col gap-1 text-sm">
          <label htmlFor={extId}>{kind === "directory" ? "Group's full DN" : "Group name in the provider's tokens"}</label>
          <input id={extId} className="input input-sm w-full font-mono" value={external} onChange={(e) => setExternal(e.target.value)} autoComplete="off" spellCheck={false} placeholder={kind === "directory" ? "cn=ops,ou=groups,dc=example,dc=org" : "ops-oncall"} data-testid="group-external" />
          <span className="text-xs text-base-content/60">{kind === "directory" ? "The whole DN, not only the group's name: a group of the same name elsewhere in the directory is a different group." : "Members then come from the provider and cannot be changed here."}</span>
        </div>
      )}
    </>
  );
}

function CreateGroupDialog({ open, me, dir, sources, onAuth, onClose, onCreated }: { open: boolean; me: WhoAmI | null; dir: Directory; sources: GroupSource[]; onAuth: (m: string) => void; onClose: () => void; onCreated: (g: GroupDto) => void }) {
  return (
    <FormDialog open={open} title="Create a group" onClose={onClose} testId="group-dialog">
      {open && <CreateGroupForm me={me} dir={dir} sources={sources} onAuth={onAuth} onClose={onClose} onCreated={onCreated} />}
    </FormDialog>
  );
}

function CreateGroupForm({ me, dir, sources, onAuth, onClose, onCreated }: { me: WhoAmI | null; dir: Directory; sources: GroupSource[]; onAuth: (m: string) => void; onClose: () => void; onCreated: (g: GroupDto) => void }) {
  const platform = holdsPlatform(me, "group.manage");
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [orgId, setOrgId] = useState(platform ? "" : "-");
  const [source, setSource] = useState("");
  const [external, setExternal] = useState("");
  const { busy, error, run } = useRun(onAuth);
  const nameId = useId();
  const descId = useId();
  const orgSel = useId();
  const orgs = dir.orgs.kind === "ok" ? dir.orgs.data : [];
  const nameOk = name.trim().length >= 1 && name.trim().length <= 80;
  const orgChosen = orgId !== "-";
  const mapped = platform && source !== "" && external.trim() !== "";
  const submit = () => {
    void run(async () => {
      const g = await client.request("create_group", { body: { name: name.trim(), description: description.trim(), organizationId: orgId && orgChosen ? orgId : null, externalName: mapped ? external.trim() : null, externalSource: mapped ? source : null } });
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
        <ExternalFields sources={sources} source={source} setSource={setSource} external={external} setExternal={setExternal} optional />
      ) : (
        <p className="text-xs text-base-content/60">Taking a group&apos;s members from an identity provider or directory is for platform administrators.</p>
      )}
      {error && <ErrorPanel message={error} testId="group-error" />}
      <div className="modal-action mt-0">
        <button type="button" className="btn btn-sm btn-ghost" onClick={onClose}>Cancel</button>
        <button type="submit" className="btn btn-sm btn-primary" disabled={busy || !nameOk || !orgChosen} data-testid="group-submit">{busy ? "Creating…" : "Create group"}</button>
      </div>
    </form>
  );
}

function SetSourceDialog({ group, sources, onAuth, onClose, onSaved }: { group: GroupDto | null; sources: GroupSource[]; onAuth: (m: string) => void; onClose: () => void; onSaved: (g: GroupDto) => void }) {
  return (
    <FormDialog open={!!group} title={group ? `Where do members of ${group.name} come from?` : ""} onClose={onClose} testId="group-source-dialog">
      {group && <SetSourceForm group={group} sources={sources} onAuth={onAuth} onClose={onClose} onSaved={onSaved} />}
    </FormDialog>
  );
}

function SetSourceForm({ group, sources, onAuth, onClose, onSaved }: { group: GroupDto; sources: GroupSource[]; onAuth: (m: string) => void; onClose: () => void; onSaved: (g: GroupDto) => void }) {
  const [source, setSource] = useState(sources.length === 1 ? (sources[0]?.id ?? "") : "");
  const [external, setExternal] = useState(group.externalName ?? "");
  const { busy, error, run } = useRun(onAuth);
  const submit = () => {
    void run(async () => {
      onSaved(await client.request("set_group_external", { params: { group: group.id }, body: { source, name: external.trim() } }));
    });
  };
  return (
    <form className="flex flex-col gap-3" onSubmit={(e) => { e.preventDefault(); submit(); }}>
      <p className="text-sm text-base-content/70">This group was linked to the group <span className="font-mono">{group.externalName}</span> before Apelora recorded which identity provider or directory it belongs to. Until you choose one, nobody is a member through it.</p>
      <ExternalFields sources={sources} source={source} setSource={setSource} external={external} setExternal={setExternal} optional={false} />
      {error && <ErrorPanel message={error} testId="group-source-error" />}
      <div className="modal-action mt-0">
        <button type="button" className="btn btn-sm btn-ghost" onClick={onClose}>Cancel</button>
        <button type="submit" className="btn btn-sm btn-primary" disabled={busy} data-testid="group-source-submit">{busy ? "Saving…" : "Save source"}</button>
      </div>
    </form>
  );
}

function GroupMembers({ group, me, orgName, sources, onAuth }: { group: GroupDto; me: WhoAmI | null; orgName: string; sources: GroupSource[]; onAuth: (m: string) => void }) {
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
            {managed && group.needsSource && (
              <Notice tone="warn" testId="group-managed-note">
                This group is linked to the group <span className="font-mono">{group.externalName}</span> but not to an identity provider or directory, so nobody is a member through it. A platform administrator chooses its source with Set source.
              </Notice>
            )}
            {managed && !group.needsSource && (
              <Notice tone="info" testId="group-managed-note">
                Members of this group come from {sourceName(sources, group.externalSource)} (group <span className="font-mono">{group.externalName}</span>), so they are listed here but cannot be added or removed here. Change them there; they appear after their next sign-in.
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
  const [loaded, reload] = useLoad<GroupList>(() => client.get("groups"), onAuth);
  const groups: Load<GroupDto[]> = loaded.kind === "ok" ? { kind: "ok", data: loaded.data.items } : loaded;
  // Older control planes do not send sources (one version of skew, ADR 0011).
  const sources: GroupSource[] = (loaded.kind === "ok" ? loaded.data.sources : undefined) ?? [];
  const dir = useDirectory(me, onAuth, { orgs: true });
  const [creating, setCreating] = useState(false);
  const [selected, setSelected] = useState<string | null>(null);
  const [assign, setAssign] = useState<AssignPreset | null>(null);
  const [sourcing, setSourcing] = useState<GroupDto | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const canAssign = holds(me, "role.assign");
  const canMap = holdsPlatform(me, "group.manage");
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
                      <td className="max-sm:hidden">{g.needsSource ? <Badge tone="warn" title={`Linked to ${g.externalName ?? ""} without a source: nobody is a member through it`} testId="group-needs-source">Needs a source</Badge> : g.externalName ? <Badge tone="info" title={`Group ${g.externalName}`}>{sourceName(sources, g.externalSource)}</Badge> : <Badge>Managed here</Badge>}</td>
                      <td className="whitespace-nowrap text-right">
                        {g.needsSource && canMap && <button type="button" className="btn btn-xs btn-ghost" aria-label={`Set the source of ${g.name}`} onClick={() => setSourcing(g)} data-testid="group-set-source">Set source…</button>}
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

      {picked && <GroupMembers key={picked.id} group={picked} me={me} orgName={picked.organizationId ? dir.names.org(picked.organizationId) : ""} sources={sources} onAuth={onAuth} />}

      <CreateGroupDialog open={creating} me={me} dir={dir} sources={sources} onAuth={onAuth} onClose={() => setCreating(false)} onCreated={(g) => { setCreating(false); setNotice(`Group ${g.name} created.`); setSelected(g.id); reload(); }} />
      <SetSourceDialog group={sourcing} sources={sources} onAuth={onAuth} onClose={() => setSourcing(null)} onSaved={(g) => { setSourcing(null); setNotice(`Members of ${g.name} now come from ${sourceName(sources, g.externalSource)}.`); reload(); }} />
      <AssignmentDialog open={!!assign} preset={assign ?? {}} me={me} onAuth={onAuth} onClose={() => setAssign(null)} onSaved={(m) => { setAssign(null); setNotice(m); }} />
    </>
  );
}
