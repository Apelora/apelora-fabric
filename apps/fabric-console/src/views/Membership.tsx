import { useState } from "react";

import type { JoinTokenCreated, JoinTokenList } from "@apelora/api-client";

import { ConfirmDialog, ErrorPanel, Section, SectionHeader, TableFrame } from "../components/ui";
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

export function MembershipView({ onAuth }: { onAuth: (m: string) => void }) {
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
