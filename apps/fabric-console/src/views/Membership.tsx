import { useEffect, useState } from "react";

import type { EvacuationPreview, JoinTokenList, RemovalPreview } from "@apelora/api-client";

import { ConfirmDialog, ErrorPanel, FormDialog, Section, SectionHeader, TableFrame, sentence } from "../components/ui";
import { client, messageOf, useLoad } from "../lib/api";
import { MachinesSection } from "./Machines";
import { MaasMachinesSection } from "./MaasMachines";

/**
 * Evacuating a member for maintenance: first the dry run (what Incus does with each instance
 * and what that means for its application), then the member's name typed to confirm.
 */
export function EvacuateDialog({ member, onClose, onStarted }: { member: string | null; onClose: () => void; onStarted: (message: string) => void }) {
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

/**
 * Removing an evacuated member for good: the check runs first (nothing may be left on it, the
 * cluster must keep running), then the member's name typed to confirm.
 */
export function RemoveMemberDialog({ member, onClose, onRemoved }: { member: string | null; onClose: () => void; onRemoved: (message: string) => void }) {
  const [preview, setPreview] = useState<RemovalPreview | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [typed, setTyped] = useState("");
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    if (!member) return;
    let current = true;
    setPreview(null);
    setError(null);
    setTyped("");
    client.request("preview_member_removal", { params: { member } }).then(
      (p) => { if (current) setPreview(p); },
      (e: unknown) => { if (current) setError(messageOf(e)); },
    );
    return () => { current = false; };
  }, [member]);
  const plan = preview?.plan;
  const blocked = !!plan?.blocked.length;
  const remove = async () => {
    if (!member) return;
    setBusy(true);
    setError(null);
    try {
      await client.request("remove_member", { params: { member }, body: { confirm: typed } });
      onRemoved(`${member} was removed from the cluster. To use the server again, reinstall it and enrol it as a new machine.`);
    } catch (e) {
      setError(messageOf(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <FormDialog open={!!member} title={`Remove ${member ?? ""} from the cluster?`} onClose={onClose} testId="remove-member-dialog">
      <p className="text-sm text-base-content/80">It stops being a cluster member for good. This cannot be undone.</p>
      {!plan && !error && <p className="text-sm text-base-content/60" role="status">Checking what is still on it…</p>}
      {plan && (
        <>
          {plan.blocked.map((b) => <p key={b} role="alert" className="text-sm text-error" data-testid="removal-blocked">Cannot remove: {b}</p>)}
          {!blocked && plan.warnings.map((w) => <p key={w} className="flex items-start gap-2 text-sm" role="note" data-testid="removal-warning"><span className="status status-warning mt-1.5 shrink-0" aria-hidden="true" />{sentence(w)}</p>)}
          {!blocked && (
            <label className="flex flex-col gap-1 text-sm">
              <span>Type <strong className="font-mono">{member}</strong> to confirm</span>
              <input className="input input-sm w-full font-mono" autoComplete="off" spellCheck={false} value={typed} onChange={(e) => setTyped(e.target.value)} data-testid="remove-member-confirm" />
            </label>
          )}
        </>
      )}
      {error && <ErrorPanel message={error} testId="remove-member-error" />}
      <div className="modal-action mt-0">
        <button type="button" className="btn btn-sm btn-outline" onClick={onClose}>Cancel</button>
        <button type="button" className="btn btn-sm btn-ghost text-error" disabled={!plan || blocked || typed !== member || busy} onClick={() => void remove()} data-testid="remove-member-start">
          {busy ? "Removing…" : "Remove member"}
        </button>
      </div>
    </FormDialog>
  );
}


/** Servers → Machines and join tokens: enrolment of new servers and the tokens still unused. */
export function MachinesTab({ onAuth }: { onAuth: (m: string) => void }) {
  const [tokens, reload] = useLoad<JoinTokenList>(() => client.get("list_join_tokens"), onAuth);
  const [error, setError] = useState<string | null>(null);
  const [toRevoke, setToRevoke] = useState<string | null>(null);

  const revoke = async (member: string) => {
    setError(null);
    try {
      await client.request("revoke_join_token", { params: { member } });
      reload();
    } catch (e) {
      setError(messageOf(e));
    }
  };

  return (
    <div className="flex flex-col gap-6">
      <MachinesSection onAuth={onAuth} onChanged={reload} />
      <MaasMachinesSection onAuth={onAuth} onChanged={reload} />
      {error && <ErrorPanel message={error} testId="membership-error" />}

      <Section title="Unused join tokens" state={tokens} testId="join-tokens" onRetry={reload}>
        {(l) => (
          <>
            <SectionHeader title="Unused join tokens" count={l.items.length} source={l.source} />
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
        body={<p>The server can no longer join with it. Approve the machine again for a new token.</p>}
        action="Revoke token"
        onCancel={() => setToRevoke(null)}
        onConfirm={() => { const m = toRevoke; setToRevoke(null); if (m) void revoke(m); }}
      />
    </div>
  );
}
