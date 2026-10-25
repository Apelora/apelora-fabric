import { useEffect, useId, useState, type KeyboardEvent } from "react";

import type { DirectoryCheck, DirectoryInput, DirectoryProblem, DirectoryView } from "@apelora/api-client";

import { Badge, ConfirmDialog, ErrorPanel, FormDialog, Notice, TableFrame, sentence } from "../../components/ui";
import { client } from "../../lib/api";
import { useRun } from "../../lib/identity";

/** Plain words for what went wrong when Apelora talked to a directory. */
const PROBLEM: Record<DirectoryProblem, string> = {
  unreachable: "Nothing answers at this address",
  timeout: "The directory did not answer in time",
  tls: "The TLS connection failed",
  "certificate-untrusted": "The certificate is not trusted",
  "certificate-name-mismatch": "The certificate is for another host name",
  "start-tls-refused": "The directory refused StartTLS",
  "bind-refused": "Wrong username or password",
  "entry-not-found": "Signed in, but the person's entry was not found",
  "entry-ambiguous": "The user filter found more than one person",
  "search-failed": "A search in the directory failed",
  settings: "These settings cannot be used",
};

const blank = (s: string) => (s.trim() ? s.trim() : null);

/** Whether the address is one Apelora accepts: ldaps://, or ldap:// with StartTLS. */
function addressProblem(url: string, startTls: boolean): string | null {
  const u = url.trim().toLowerCase();
  if (!u) return null;
  if (u.startsWith("ldaps://")) return u.length > 8 ? null : "Add the host name.";
  if (u.startsWith("ldap://")) return startTls ? null : "Plain LDAP sends passwords unencrypted: turn on StartTLS below or use ldaps://.";
  return "Start with ldaps:// (or ldap:// with StartTLS).";
}

type Form = {
  name: string; url: string; startTls: boolean; caPem: string; bindTemplate: string; baseDn: string; userFilter: string;
  idAttribute: string; usernameAttribute: string; emailAttribute: string; nameAttribute: string;
  groupAttribute: string; groupBaseDn: string; groupFilter: string; groupNameAttribute: string; enabled: boolean;
};

const EMPTY: Form = {
  name: "", url: "", startTls: false, caPem: "", bindTemplate: "", baseDn: "", userFilter: "", idAttribute: "entryUUID",
  usernameAttribute: "uid", emailAttribute: "mail", nameAttribute: "cn", groupAttribute: "memberOf", groupBaseDn: "", groupFilter: "",
  groupNameAttribute: "cn", enabled: true,
};

const formOf = (d: DirectoryView): Form => ({
  name: d.name, url: d.url, startTls: d.startTls, caPem: d.caPem ?? "", bindTemplate: d.bindTemplate, baseDn: d.baseDn ?? "",
  userFilter: d.userFilter ?? "", idAttribute: d.idAttribute, usernameAttribute: d.usernameAttribute, emailAttribute: d.emailAttribute,
  nameAttribute: d.nameAttribute, groupAttribute: d.groupAttribute ?? "", groupBaseDn: d.groupBaseDn ?? "", groupFilter: d.groupFilter ?? "",
  groupNameAttribute: d.groupNameAttribute, enabled: d.enabled,
});

const inputOf = (f: Form): DirectoryInput => ({
  name: f.name.trim(), url: f.url.trim(), startTls: f.url.trim().toLowerCase().startsWith("ldap://") && f.startTls, caPem: blank(f.caPem),
  bindTemplate: f.bindTemplate.trim(), baseDn: blank(f.baseDn), userFilter: blank(f.userFilter), idAttribute: blank(f.idAttribute),
  usernameAttribute: blank(f.usernameAttribute), emailAttribute: blank(f.emailAttribute), nameAttribute: blank(f.nameAttribute),
  groupAttribute: blank(f.groupAttribute), groupBaseDn: blank(f.groupBaseDn), groupFilter: blank(f.groupFilter),
  groupNameAttribute: blank(f.groupNameAttribute), enabled: f.enabled,
});

/** How a directory's groups are read, in words for the table. */
function groupsIn(d: DirectoryView): string {
  const parts = [d.groupAttribute ? `from ${d.groupAttribute}` : null, d.groupBaseDn ? `by search under ${d.groupBaseDn}` : null].filter(Boolean);
  return parts.length ? `Groups ${parts.join(" and ")}` : "No groups read";
}

/**
 * A test sign-in with a username and password typed for this check only: they are sent once,
 * never saved, and cleared when the dialog closes.
 */
function CheckPanel({ settings, directory, onAuth, ready }: { settings: () => DirectoryInput; directory: string | null; onAuth: (m: string) => void; ready: boolean }) {
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [result, setResult] = useState<DirectoryCheck | null>(null);
  const run = useRun(onAuth);
  const id = useId();
  const submit = () => void run.run(async () => {
    setResult(null);
    setResult(await client.request("check_directory", { body: { settings: settings(), directory, username: username.trim(), password } }));
  });
  const canRun = ready && !!username.trim() && !!password && !run.busy;
  // Enter runs the check, never the settings form around it.
  const onEnter = (e: KeyboardEvent) => { if (e.key === "Enter") { e.preventDefault(); if (canRun) submit(); } };
  return (
    <fieldset className="flex flex-col gap-3 rounded-box border border-base-300 px-3 pt-1 pb-3" data-testid="directory-check-panel">
      <legend className="px-1 text-sm font-semibold">Test sign-in</legend>
      <p className="text-xs text-base-content/70">What Apelora reads for one person. Never saved.</p>
      <div className="grid gap-3 sm:grid-cols-[1fr_1fr_auto] sm:items-end">
        <div className="flex flex-col gap-1 text-sm">
          <label htmlFor={`${id}-u`}>Test username</label>
          <input id={`${id}-u`} className="input input-sm w-full" value={username} onChange={(e) => setUsername(e.target.value)} autoComplete="off" spellCheck={false} data-testid="directory-check-username" onKeyDown={onEnter} />
        </div>
        <div className="flex flex-col gap-1 text-sm">
          <label htmlFor={`${id}-p`}>Test password</label>
          <input id={`${id}-p`} type="password" className="input input-sm w-full" value={password} onChange={(e) => setPassword(e.target.value)} autoComplete="new-password" data-testid="directory-check-password"
            onKeyDown={onEnter} />
        </div>
        <button type="button" className="btn btn-sm btn-outline" disabled={!canRun} onClick={submit} data-testid="directory-check-run">{run.busy ? "Checking…" : "Check sign-in"}</button>
      </div>
      {run.error && <ErrorPanel message={run.error} testId="directory-check-error" />}
      {result?.ok && result.entry && (
        <Notice tone="ok" title={`Signed in as ${result.entry.displayName ?? result.entry.username}`} testId="directory-check-ok">
          <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-xs">
            <dt className="text-base-content/70">Username</dt><dd className="font-mono [overflow-wrap:anywhere]">{result.entry.username}</dd>
            <dt className="text-base-content/70">Email</dt><dd className="[overflow-wrap:anywhere]">{result.entry.email ?? "—"}</dd>
            <dt className="text-base-content/70">Entry</dt><dd className="font-mono [overflow-wrap:anywhere]">{result.entry.dn}</dd>
            <dt className="text-base-content/70">Stable ID</dt><dd className="font-mono [overflow-wrap:anywhere]">{result.entry.subject}</dd>
            <dt className="text-base-content/70">Groups</dt><dd>{result.entry.groups.length ? result.entry.groups.join(", ") : "None"}</dd>
            <dt className="text-base-content/70">Apelora groups</dt>
            <dd data-testid="directory-check-mapped">{result.entry.mappedGroups.length ? result.entry.mappedGroups.join(", ") : directory ? "None match yet: under Groups, take a group's members from this directory with one of these DNs." : "Save the directory first; then, under Groups, take a group's members from it with one of these DNs."}</dd>
          </dl>
        </Notice>
      )}
      {result && !result.ok && (
        <Notice tone="error" title={result.problem ? PROBLEM[result.problem] : "The check failed"} testId="directory-check-failed">{sentence(result.message)}.</Notice>
      )}
    </fieldset>
  );
}

function DirectoryDialog({ open, editing, onAuth, onClose, onSaved }: { open: boolean; editing: DirectoryView | null; onAuth: (m: string) => void; onClose: () => void; onSaved: () => void }) {
  const [f, setF] = useState<Form>(EMPTY);
  const save = useRun(onAuth);
  const id = useId();
  const set = <K extends keyof Form>(k: K) => (v: Form[K]) => setF((x) => ({ ...x, [k]: v }));

  useEffect(() => {
    if (!open) return;
    setF(editing ? formOf(editing) : EMPTY);
    save.setError(null);
  }, [open, editing]); // eslint-disable-line react-hooks/exhaustive-deps

  const plain = f.url.trim().toLowerCase().startsWith("ldap://");
  const urlProblem = addressProblem(f.url, f.startTls);
  const templateProblem = f.bindTemplate.trim() && !f.bindTemplate.includes("{username}") ? "Put {username} where the person's username goes." : null;
  const ok = !!f.name.trim() && !!f.url.trim() && !urlProblem && !!f.bindTemplate.trim() && !templateProblem;
  const submit = () => void save.run(async () => {
    if (editing) await client.request("update_directory", { params: { id: editing.id }, body: inputOf(f) });
    else await client.request("create_directory", { body: inputOf(f) });
    onSaved();
  });
  const field = (k: keyof Form, label: string, opts: { placeholder?: string; hint?: string; mono?: boolean; testId?: string } = {}) => (
    <div className="flex flex-col gap-1 text-sm">
      <label htmlFor={`${id}-${k}`}>{label}</label>
      <input id={`${id}-${k}`} className={`input input-sm w-full ${opts.mono === false ? "" : "font-mono"}`} value={f[k] as string} onChange={(e) => set(k)(e.target.value as never)}
        placeholder={opts.placeholder} spellCheck={false} autoComplete="off" data-testid={opts.testId ?? `directory-${k}`} aria-describedby={opts.hint ? `${id}-${k}-hint` : undefined} />
      {opts.hint && <span id={`${id}-${k}-hint`} className="text-xs text-base-content/60">{opts.hint}</span>}
    </div>
  );

  return (
    <FormDialog open={open} title={editing ? `Edit ${editing.name}` : "Add a directory"} onClose={onClose} testId="directory-dialog" wide>
      <form className="flex flex-col gap-3" onSubmit={(e) => { e.preventDefault(); if (ok && !save.busy) submit(); }}>
        <p className="text-sm text-base-content/70">Apelora signs in to the directory as the person, over TLS, and stores no directory password.</p>
        <div className="grid gap-3 sm:grid-cols-2">
          <div className="flex flex-col gap-1 text-sm">
            <label htmlFor={`${id}-name`}>Name on the sign-in form</label>
            <input id={`${id}-name`} className="input input-sm w-full" value={f.name} onChange={(e) => set("name")(e.target.value)} maxLength={60} placeholder="Staff directory" data-testid="directory-name" />
          </div>
          <div className="flex flex-col gap-1 text-sm">
            <label htmlFor={`${id}-url`}>Address</label>
            <input id={`${id}-url`} className="input input-sm w-full font-mono" value={f.url} onChange={(e) => set("url")(e.target.value)} placeholder="ldaps://ldap.example.org" spellCheck={false} autoComplete="off"
              aria-invalid={!!urlProblem} aria-describedby={urlProblem ? `${id}-url-error` : undefined} data-testid="directory-url" />
            {urlProblem && <span id={`${id}-url-error`} className="text-xs text-error" data-testid="directory-url-error">{urlProblem}</span>}
          </div>
        </div>
        {plain && (
          <label className="flex items-center gap-2 text-sm">
            <input type="checkbox" className="checkbox checkbox-sm" checked={f.startTls} onChange={(e) => set("startTls")(e.target.checked)} data-testid="directory-start-tls" />
            Use StartTLS (switch this ldap:// connection to TLS before signing in)
          </label>
        )}
        <div className="flex flex-col gap-1 text-sm">
          <label htmlFor={`${id}-bind`}>Sign-in name</label>
          <input id={`${id}-bind`} className="input input-sm w-full font-mono" value={f.bindTemplate} onChange={(e) => set("bindTemplate")(e.target.value)} placeholder="uid={username},ou=people,dc=example,dc=org" spellCheck={false} autoComplete="off"
            aria-invalid={!!templateProblem} aria-describedby={`${id}-bind-hint`} data-testid="directory-bind" />
          <span id={`${id}-bind-hint`} className={`text-xs ${templateProblem ? "text-error" : "text-base-content/60"}`}>
            {templateProblem ?? "{username} is what the person types. For {username}@example.org, also set where to find people."}
          </span>
        </div>
        <div className="flex flex-col gap-1 text-sm">
          <label htmlFor={`${id}-ca`}>Certificate authority (PEM, optional)</label>
          <textarea id={`${id}-ca`} className="textarea textarea-sm w-full font-mono text-xs" rows={3} value={f.caPem} onChange={(e) => set("caPem")(e.target.value)} placeholder="-----BEGIN CERTIFICATE-----" spellCheck={false}
            aria-describedby={`${id}-ca-hint`} data-testid="directory-ca" />
          <span id={`${id}-ca-hint`} className="text-xs text-base-content/60">Empty: this server's trusted authorities. The host name is always checked.</span>
        </div>
        <details className="rounded-box border border-base-300 px-3 py-2 text-sm" data-testid="directory-advanced">
          <summary className="cursor-pointer select-none">Finding people and groups</summary>
          <div className="mt-3 flex flex-col gap-3">
            <div className="grid gap-3 sm:grid-cols-2">
              {field("baseDn", "Search people under", { placeholder: "ou=people,dc=example,dc=org" })}
              {field("userFilter", "User filter", { placeholder: "(uid={username})", hint: "Empty: the sign-in name is the person's entry." })}
              {field("idAttribute", "Stable ID attribute", { hint: "entryUUID; objectGUID is read as hex." })}
              {field("usernameAttribute", "Username attribute")}
              {field("emailAttribute", "Email attribute")}
              {field("nameAttribute", "Display name attribute")}
            </div>
            <div className="grid gap-3 sm:grid-cols-2">
              {field("groupAttribute", "Group attribute of the person", { hint: "Such as memberOf. Empty: not read." })}
              {field("groupBaseDn", "Search groups under", { placeholder: "ou=groups,dc=example,dc=org", hint: "Empty: no group search." })}
              {field("groupFilter", "Group filter", { placeholder: "(member={dn})", hint: "{dn} is the person's entry." })}
              {field("groupNameAttribute", "Group name attribute")}
            </div>
            <p className="text-xs text-base-content/60">Directory groups map onto Apelora groups by external name (Identity &amp; Access → Groups).</p>
          </div>
        </details>
        <label className="flex items-center gap-2 text-sm">
          <input type="checkbox" className="checkbox checkbox-sm" checked={f.enabled} onChange={(e) => set("enabled")(e.target.checked)} data-testid="directory-enabled" />
          Offer this directory on the sign-in page
        </label>
        {open && <CheckPanel settings={() => inputOf(f)} directory={editing?.id ?? null} onAuth={onAuth} ready={ok} />}
        {save.error && <ErrorPanel message={save.error} testId="directory-save-error" />}
        <div className="modal-action mt-0">
          <button type="button" className="btn btn-sm btn-outline" onClick={onClose}>Cancel</button>
          <button type="submit" className="btn btn-sm btn-primary" disabled={!ok || save.busy} data-testid="directory-save">{save.busy ? "Saving…" : editing ? "Save changes" : "Add directory"}</button>
        </div>
      </form>
    </FormDialog>
  );
}

/**
 * Identity & Access → Sign-in → Directories: LDAP directories (over TLS) people sign in with.
 * Add, edit (with a test sign-in), turn off and remove; the server validates and audits every change.
 */
export function DirectoriesSection({ directories, onAuth, reload }: { directories: DirectoryView[]; onAuth: (m: string) => void; reload: () => void }) {
  const [dialog, setDialog] = useState<{ editing: DirectoryView | null } | null>(null);
  const [toDelete, setToDelete] = useState<DirectoryView | null>(null);
  const action = useRun(onAuth);

  const toggle = (d: DirectoryView) => void action.run(async () => {
    await client.request("update_directory", { params: { id: d.id }, body: { ...inputOf(formOf(d)), enabled: !d.enabled } });
    reload();
  });
  const remove = (d: DirectoryView) => void action.run(async () => {
    await client.request("delete_directory", { params: { id: d.id }, body: { confirm: d.name } });
    reload();
  });

  return (
    <section className="flex flex-col gap-3" aria-labelledby="directories-heading" data-testid="directories">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h2 id="directories-heading" className="text-sm font-semibold">Directories <span className="font-normal text-base-content/60">({directories.length})</span></h2>
        <button type="button" className="btn btn-sm btn-outline" onClick={() => setDialog({ editing: null })} data-testid="directory-add">Add directory</button>
      </div>
      {action.error && <ErrorPanel message={action.error} testId="directories-error" />}
      {directories.length === 0 ? (
        <p className="text-sm text-base-content/70" data-testid="no-directories">No directory yet. Add one so people sign in with their existing LDAP username and password.</p>
      ) : (
        <TableFrame>
          <thead><tr><th>Directory</th><th className="max-sm:hidden">Groups</th><th>Status</th><th><span className="sr-only">Actions</span></th></tr></thead>
          <tbody>
            {directories.map((d) => (
              <tr key={d.id} data-testid="directory-row">
                <td className="font-medium">{d.name}<span className="block font-mono text-xs font-normal text-base-content/60 [overflow-wrap:anywhere]">{d.url}{d.startTls ? " (StartTLS)" : ""}</span></td>
                <td className="max-sm:hidden text-xs text-base-content/70 [overflow-wrap:anywhere]">{groupsIn(d)}</td>
                <td className="text-sm">
                  {!d.enabled ? <Badge tone="neutral">Off</Badge> : d.reachable ? <Badge tone="ok">Answers</Badge> : <Badge tone="error">{d.problem ? PROBLEM[d.problem] : "Not reachable"}</Badge>}
                  {d.enabled && !d.reachable && d.error && <span className="mt-1 block text-xs text-error">{sentence(d.error)}</span>}
                </td>
                <td className="text-right">
                  <span className="inline-flex flex-wrap justify-end gap-1 lg:flex-nowrap">
                    <button type="button" className="btn btn-xs btn-ghost" aria-label={`Edit ${d.name}`} onClick={() => setDialog({ editing: d })}>Edit…</button>
                    <button type="button" className="btn btn-xs btn-ghost" aria-label={`${d.enabled ? "Turn off" : "Turn on"} ${d.name}`} disabled={action.busy} onClick={() => toggle(d)}>{d.enabled ? "Turn off" : "Turn on"}</button>
                    <button type="button" className="btn btn-xs btn-ghost text-error" aria-label={`Remove ${d.name}`} onClick={() => setToDelete(d)}>Remove…</button>
                  </span>
                </td>
              </tr>
            ))}
          </tbody>
        </TableFrame>
      )}
      <DirectoryDialog open={!!dialog} editing={dialog?.editing ?? null} onAuth={onAuth} onClose={() => setDialog(null)} onSaved={() => { setDialog(null); reload(); }} />
      <ConfirmDialog
        open={!!toDelete}
        title={`Remove ${toDelete?.name ?? ""}?`}
        body={<p>People can no longer sign in with this directory. Their accounts and roles in Apelora stay.</p>}
        action="Remove directory"
        typeToConfirm={toDelete?.name}
        onCancel={() => setToDelete(null)}
        onConfirm={() => { const d = toDelete; setToDelete(null); if (d) remove(d); }}
      />
    </section>
  );
}
