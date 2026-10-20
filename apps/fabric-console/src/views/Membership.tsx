import { useEffect, useState } from "react";

import type { ClusterMember, ClusterResponse, EvacuationPreview, JoinTokenCreated, JoinTokenList } from "@apelora/api-client";

import { ConfirmDialog, ErrorPanel, FormDialog, Section, SectionHeader, TableFrame } from "../components/ui";
import { client, messageOf, useLoad } from "../lib/api";

const NAME = /^[A-Za-z](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?$/;

/** Shows a new join token once. It is not kept anywhere: dismissing it discards it. */
function IssuedToken({ issued, onDismiss }: { issued: JoinTokenCreated; onDismiss: () => void }) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    try { await navigator.clipboard.writeText(issued.token); setCopied(true); } catch { setCopied(false); }
  };
  return (
    <section className="flex flex-col gap-3 rounded-box border border-base-300 p-4" aria-label="New join token" data-testid="issued-token">
      <h3 className="font-semibold">Join token for {issued.memberName}</h3>
      <p className="text-sm text-base-content/70">
        Shown only once. On the new server, run <code className="rounded bg-base-200 px-1 py-0.5 text-xs">incus admin init</code> and paste it. Valid once, until {new Date(issued.expiresAt).toLocaleString()}.
      </p>
      <textarea readOnly className="textarea textarea-sm w-full font-mono text-xs" rows={3} value={issued.token} aria-label="Join token" data-testid="issued-token-value" onFocus={(e) => e.currentTarget.select()} />
      {issued.warnings.map((w) => <p key={w} className="flex items-start gap-2 text-sm" role="note"><span className="status status-warning mt-1.5 shrink-0" aria-hidden="true" />{w}</p>)}
      <div className="flex gap-2">
        <button type="button" className="btn btn-sm btn-outline" onClick={() => void copy()}>{copied ? "Copied" : "Copy token"}</button>
        <button type="button" className="btn btn-sm btn-ghost" onClick={onDismiss}>Hide token</button>
      </div>
    </section>
  );
}

/** The API's messages are clauses; shown on their own they start with a capital. */
const sentence = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);

/**
 * Evacuating a member for maintenance: first the dry run (what Incus does with each instance
 * and what that means for its application), then the member's name typed to confirm.
 */
function EvacuateDialog({ member, onClose, onStarted }: { member: string | null; onClose: () => void; onStarted: (message: string) => void }) {
  const [preview, setPreview] = useState<EvacuationPreview | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [typed, setTyped] = useState("");
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    if (!member) return;
    let current = true;
    setPreview(null);
    setError(null);
    setTyped("");
    client.request("preview_evacuation", { params: { member } }).then(
      (p) => { if (current) setPreview(p); },
      (e: unknown) => { if (current) setError(messageOf(e)); },
    );
    return () => { current = false; };
  }, [member]);
  const plan = preview?.plan;
  const blocked = !!plan?.blocked.length;
  const start = async () => {
    if (!member) return;
    setBusy(true);
    setError(null);
    try {
      await client.request("evacuate_member", { params: { member }, body: { confirm: typed } });
      onStarted(`Evacuating ${member}: Incus is moving or stopping its instances. Restore the member when maintenance is done.`);
    } catch (e) {
      setError(messageOf(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <FormDialog open={!!member} title={`Evacuate ${member ?? ""}?`} onClose={onClose} testId="evacuate-dialog" wide>
      <p className="text-sm text-base-content/80">
        Its instances move to other members, or stop, until you restore it.
      </p>
      {!plan && !error && <p className="text-sm text-base-content/60" role="status">Checking what would happen…</p>}
      {plan && (
        <>
          {plan.blocked.map((b) => <p key={b} role="alert" className="text-sm text-error" data-testid="evacuation-blocked">Cannot evacuate: {b}</p>)}
          {plan.warnings.map((w) => <p key={w} className="flex items-start gap-2 text-sm" role="note" data-testid="evacuation-warning"><span className="status status-warning mt-1.5 shrink-0" aria-hidden="true" />{sentence(w)}</p>)}
          {plan.items.length === 0 ? <p className="text-sm text-base-content/70">No instances run on this member.</p> : (
            <TableFrame>
              <thead><tr><th>Instance</th><th>What Incus does</th><th>Effect</th></tr></thead>
              <tbody>
                {plan.items.map((i) => (
                  <tr key={`${i.project}/${i.instance}`} data-testid="evacuation-item">
                    <td className="font-medium break-words sm:whitespace-nowrap">{i.instance}<span className="block text-xs font-normal text-base-content/60">{i.project}{i.application ? ` · ${i.application}` : ""}</span></td>
                    <td className="text-sm">{sentence(i.outcome)}</td>
                    <td className="text-sm">{sentence(i.impact)}</td>
                  </tr>
                ))}
              </tbody>
            </TableFrame>
          )}
          {!blocked && (
            <label className="flex flex-col gap-1 text-sm">
              <span>Type <strong className="font-mono">{member}</strong> to confirm</span>
              <input className="input input-sm w-full font-mono" autoComplete="off" spellCheck={false} value={typed} onChange={(e) => setTyped(e.target.value)} data-testid="evacuate-confirm" />
            </label>
          )}
        </>
      )}
      {error && <ErrorPanel message={error} testId="evacuate-error" />}
      <div className="modal-action mt-0">
        <button type="button" className="btn btn-sm btn-outline" onClick={onClose}>Cancel</button>
        <button type="button" className="btn btn-sm btn-ghost text-error" disabled={!plan || blocked || typed !== member || busy} onClick={() => void start()} data-testid="evacuate-start">
          {busy ? "Starting…" : "Evacuate member"}
        </button>
      </div>
    </FormDialog>
  );
}

function MemberRows({ members, onEvacuate, onRestore }: { members: ClusterMember[]; onEvacuate: (m: string) => void; onRestore: (m: string) => void }) {
  return (
    <TableFrame>
      <thead><tr><th>Member</th><th>Status</th><th>Architecture</th><th><span className="sr-only">Actions</span></th></tr></thead>
      <tbody>
        {members.map((m) => (
          <tr key={m.name} data-testid="membership-member">
            <td className="font-medium">{m.name}</td>
            <td title={m.message}><span className={`status ${m.status === "Online" ? "status-success" : m.status === "Evacuated" ? "status-warning" : "status-error"} mr-2`} aria-hidden="true" />{m.status}</td>
            <td>{m.architecture}</td>
            <td className="text-right">
              {m.status === "Evacuated"
                ? <button type="button" className="btn btn-xs btn-ghost" onClick={() => onRestore(m.name)} data-testid="restore-member">Restore…</button>
                : <button type="button" className="btn btn-xs btn-ghost" disabled={m.status !== "Online"} onClick={() => onEvacuate(m.name)} data-testid="evacuate-member">Evacuate…</button>}
            </td>
          </tr>
        ))}
      </tbody>
    </TableFrame>
  );
}

export function MembershipView({ onAuth }: { onAuth: (m: string) => void }) {
  const [cluster, reloadCluster] = useLoad<ClusterResponse>(() => client.get("cluster"), onAuth);
  const [toEvacuate, setToEvacuate] = useState<string | null>(null);
  const [toRestore, setToRestore] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const restore = async (member: string) => {
    setError(null);
    try {
      await client.request("restore_member", { params: { member }, body: { confirm: member } });
      setNotice(`Restoring ${member}: Incus moves its instances back and starts them.`);
      reloadCluster();
    } catch (e) {
      setError(messageOf(e));
    }
  };
  const [tokens, reload] = useLoad<JoinTokenList>(() => client.get("list_join_tokens"), onAuth);
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [issued, setIssued] = useState<JoinTokenCreated | null>(null);
  const [toRevoke, setToRevoke] = useState<string | null>(null);
  const nameError = name && !NAME.test(name) ? "Use 1–63 letters, digits and hyphens, starting with a letter." : null;

  const create = async () => {
    setError(null);
    setBusy(true);
    try {
      setIssued(await client.request("create_join_token", { body: { memberName: name } }));
      setName("");
      reload();
    } catch (e) {
      setError(messageOf(e));
    } finally {
      setBusy(false);
    }
  };

  const revoke = async (member: string) => {
    setError(null);
    try {
      await client.request("revoke_join_token", { params: { member } });
      if (issued?.memberName === member) setIssued(null);
      reload();
    } catch (e) {
      setError(messageOf(e));
    }
  };

  return (
    <div className="flex flex-col gap-6">
      <section className="flex flex-col gap-3" aria-label="Approve a new member">
        <h2 className="font-semibold">Approve a new member</h2>
        <p className="text-sm text-base-content/70">A server joins only with a one-time token issued for its exact name.</p>
        <form className="flex flex-wrap items-start gap-2" onSubmit={(e) => { e.preventDefault(); void create(); }}>
          <label className="flex flex-col gap-1 text-sm">
            <span className="sr-only">Member name</span>
            <input className="input input-sm w-64 font-mono" placeholder="server-04" value={name} onChange={(e) => setName(e.target.value)} autoComplete="off" spellCheck={false} aria-invalid={!!nameError} aria-describedby="member-name-hint" data-testid="member-name" />
            <span id="member-name-hint" className={`text-xs ${nameError ? "text-error" : "text-base-content/60"}`}>{nameError ?? "The name the new server will use in the cluster."}</span>
          </label>
          <button type="submit" className="btn btn-sm btn-primary" disabled={busy || !name || !!nameError} data-testid="create-join-token">{busy ? "Issuing…" : "Issue join token"}</button>
        </form>
        {error && <ErrorPanel message={error} testId="membership-error" />}
        {issued && <IssuedToken issued={issued} onDismiss={() => setIssued(null)} />}
      </section>

      <Section title="Cluster members" state={cluster} testId="membership-members" onRetry={reloadCluster}>
        {(c) => (
          <>
            <SectionHeader title="Cluster members" count={c.members.length} source={c.source} />
            <p className="text-sm text-base-content/70">Evacuate a member before maintenance: its instances move to the other members until you restore it.</p>
            {notice && <p role="status" className="flex items-start gap-2 text-sm" data-testid="membership-notice"><span className="status status-info mt-1.5 shrink-0" aria-hidden="true" />{notice}</p>}
            <MemberRows members={c.members} onEvacuate={setToEvacuate} onRestore={setToRestore} />
          </>
        )}
      </Section>

      <Section title="Waiting to join" state={tokens} testId="join-tokens" onRetry={reload}>
        {(l) => (
          <>
            <SectionHeader title="Waiting to join" count={l.items.length} source={l.source} />
            {l.items.length === 0 ? <p className="text-sm text-base-content/70">No unused join tokens.</p> : (
              <TableFrame>
                <thead><tr><th>Member</th><th>Token expires</th><th><span className="sr-only">Actions</span></th></tr></thead>
                <tbody>
                  {l.items.map((t) => (
                    <tr key={t.memberName} data-testid="join-token-row">
                      <td className="font-medium">{t.memberName}</td>
                      <td>{Number.isNaN(Date.parse(t.expiresAt)) ? t.expiresAt : new Date(t.expiresAt).toLocaleString()}</td>
                      <td className="text-right"><button type="button" className="btn btn-xs btn-ghost text-error" aria-label={`Revoke the join token for ${t.memberName}`} onClick={() => setToRevoke(t.memberName)}>Revoke</button></td>
                    </tr>
                  ))}
                </tbody>
              </TableFrame>
            )}
          </>
        )}
      </Section>

      <EvacuateDialog
        member={toEvacuate}
        onClose={() => setToEvacuate(null)}
        onStarted={(m) => { setToEvacuate(null); setNotice(m); reloadCluster(); }}
      />
      <ConfirmDialog
        open={!!toRestore}
        title={`Restore ${toRestore ?? ""}?`}
        body={<p>Incus moves the instances it evacuated back to this member and starts them. New instances may be placed on it again.</p>}
        action="Restore member"
        typeToConfirm={toRestore ?? undefined}
        onCancel={() => setToRestore(null)}
        onConfirm={() => { const m = toRestore; setToRestore(null); if (m) void restore(m); }}
      />
      <ConfirmDialog
        open={!!toRevoke}
        title={`Revoke the join token for ${toRevoke ?? ""}?`}
        body={<p>The server can no longer join with it. You can issue a new token later.</p>}
        action="Revoke token"
        onCancel={() => setToRevoke(null)}
        onConfirm={() => { const m = toRevoke; setToRevoke(null); if (m) void revoke(m); }}
      />
    </div>
  );
}
