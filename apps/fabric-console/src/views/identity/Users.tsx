import { useState } from "react";

import type { UserDto, WhoAmI } from "@apelora/api-client";

import { Badge, ConfirmDialog, ErrorPanel, Notice, Section, SectionHeader, TableFrame } from "../../components/ui";
import { client, useLoad } from "../../lib/api";
import { fmtTime, holds, holdsPlatform, personName, useRun } from "../../lib/identity";
import { AssignmentDialog, type AssignPreset } from "./AssignmentDialog";
import { EmergencyAccessDialog } from "./EmergencyAccessDialog";
import { PersonLookup } from "./PersonLookup";

export function UsersView({ me, onAuth }: { me: WhoAmI | null; onAuth: (m: string) => void }) {
  const [users, reload] = useLoad<UserDto[]>(() => client.get("users").then((r) => r.items), onAuth);
  const [assign, setAssign] = useState<AssignPreset | null>(null);
  const [emergency, setEmergency] = useState(false);
  const [toggle, setToggle] = useState<{ user: UserDto; disable: boolean } | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const { error, run } = useRun(onAuth);

  const canAssign = holds(me, "role.assign");
  const canManage = holdsPlatform(me, "user.manage");
  const canEmergency = holdsPlatform(me, "platform.emergency_access");

  const change = (u: UserDto, disable: boolean) => {
    setNotice(null);
    void run(async () => {
      await client.request(disable ? "disable_user" : "enable_user", { params: { user: u.id } });
      setNotice(disable ? `${personName(u)} is disabled: their tokens no longer work.` : `${personName(u)} is enabled again.`);
      reload();
    });
  };
  const assignTo = (u: UserDto) => setAssign({ principal: { ref: { type: "user", id: u.id }, label: personName(u) } });

  return (
    <>
      <Section title="Users" level={1} state={users} testId="users" onRetry={reload}>
        {(list) => (
          <>
            <SectionHeader title="Users" level={1} count={list.length}>
              <button type="button" className="btn btn-sm btn-ghost" onClick={reload}>Refresh</button>
              {canEmergency && <button type="button" className="btn btn-sm btn-outline" onClick={() => setEmergency(true)} data-testid="emergency-open">Emergency access…</button>}
            </SectionHeader>
            <p className="text-sm text-base-content/70">People who have signed in{canManage ? "" : ", as far as your roles let you see them"}. A person appears here after their first sign-in.</p>
            {notice && <Notice tone="ok" testId="users-notice">{notice}</Notice>}
            {error && <ErrorPanel message={error} testId="users-action-error" />}

            {canAssign && (
              <div className="rounded-box border border-base-300 p-4">
                <PersonLookup onAuth={onAuth} label="Find a person to give a role" actionLabel="Assign a role…" testId="users-lookup" onPick={assignTo} />
              </div>
            )}

            {list.length === 0 ? <p className="text-sm text-base-content/70" data-testid="users-empty">No one has signed in yet.</p> : (
              <TableFrame>
                <thead><tr><th>Name</th><th className="max-sm:hidden">E-mail</th><th className="max-sm:hidden">Issuer</th><th className="max-md:hidden">Last seen</th><th>Status</th><th><span className="sr-only">Actions</span></th></tr></thead>
                <tbody>
                  {list.map((u) => (
                    <tr key={u.id} data-testid="user-row" className={u.disabled ? "text-base-content/60" : undefined}>
                      <td className="font-medium">
                        {personName(u)}{me?.id === u.id && <span className="ml-1 font-normal text-base-content/60">(you)</span>}
                        {u.username && u.username !== personName(u) && <span className="block text-xs font-normal text-base-content/60">{u.username}</span>}
                      </td>
                      <td className="max-sm:hidden">{u.email ?? "—"}</td>
                      <td className="max-sm:hidden"><span className="block max-w-[14rem] truncate font-mono text-xs" title={u.issuer}>{u.issuer}</span></td>
                      <td className="whitespace-nowrap max-md:hidden">{fmtTime(u.lastSeenAt)}</td>
                      <td>{u.disabled ? <Badge tone="error">Disabled</Badge> : <Badge tone="ok">Active</Badge>}</td>
                      <td className="whitespace-nowrap text-right">
                        {canAssign && !u.disabled && <button type="button" className="btn btn-xs btn-ghost" aria-label={`Assign a role to ${personName(u)}`} onClick={() => assignTo(u)}>Assign role…</button>}
                        {canManage && me?.id !== u.id && (
                          <button type="button" className={`btn btn-xs btn-ghost ${u.disabled ? "" : "text-error"}`} aria-label={`${u.disabled ? "Enable" : "Disable"} ${personName(u)}`} onClick={() => setToggle({ user: u, disable: !u.disabled })}>{u.disabled ? "Enable…" : "Disable…"}</button>
                        )}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </TableFrame>
            )}
          </>
        )}
      </Section>

      <AssignmentDialog open={!!assign} preset={assign ?? {}} me={me} onAuth={onAuth} onClose={() => setAssign(null)} onSaved={(m) => { setAssign(null); setNotice(m); reload(); }} />
      <EmergencyAccessDialog open={emergency} onAuth={onAuth} onClose={() => setEmergency(false)} onGranted={(m) => { setEmergency(false); setNotice(m); }} />
      <ConfirmDialog
        open={!!toggle}
        title={toggle?.disable ? `Disable ${toggle ? personName(toggle.user) : ""}?` : `Enable ${toggle ? personName(toggle.user) : ""}?`}
        body={toggle?.disable
          ? <p>Their tokens stop working at once. You can enable the account again later.</p>
          : <p>They can sign in and use their tokens again.</p>}
        action={toggle?.disable ? "Disable account" : "Enable account"}
        danger={!!toggle?.disable}
        onCancel={() => setToggle(null)}
        onConfirm={() => { const t = toggle; setToggle(null); if (t) change(t.user, t.disable); }}
      />
    </>
  );
}
