import { useEffect, useState, useSyncExternalStore } from "react";

import type { Admission, Machine, MachineApproved, MachineList, MachineState } from "@apelora/api-client";

import { ConfirmDialog, ErrorPanel, FormDialog, Section, SectionHeader, TableFrame, sentence } from "../components/ui";
import { client, messageOf, useLoad } from "../lib/api";

const NAME = /^[A-Za-z](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?$/;

/** True from the `sm` breakpoint up; row actions sit in their own column there. */
const WIDE = "(min-width: 640px)";
const useWide = () => useSyncExternalStore(
  (change) => { const q = window.matchMedia(WIDE); q.addEventListener("change", change); return () => q.removeEventListener("change", change); },
  () => window.matchMedia(WIDE).matches,
);

/** Each enrolment state in words, with its status colour. */
const STATES: Record<MachineState, { label: string; dot: string }> = {
  registered: { label: "Not approved", dot: "status-neutral" },
  approved: { label: "Waiting to join", dot: "status-info" },
  "approval-expired": { label: "Token expired", dot: "status-warning" },
  held: { label: "Joined, not admitted", dot: "status-warning" },
  eligible: { label: "In service", dot: "status-success" },
  left: { label: "Left the cluster", dot: "status-neutral" },
};

const when = (t: string) => (Number.isNaN(Date.parse(t)) ? t : new Date(t).toLocaleString());

/** The join token of an approval, shown once. Dismissing it discards it. */
function ApprovedToken({ approved, onDismiss }: { approved: MachineApproved; onDismiss: () => void }) {
  const [copied, setCopied] = useState<"token" | "command" | null>(null);
  const copy = async (what: "token" | "command") => {
    try {
      await navigator.clipboard.writeText(what === "token" ? approved.token : approved.enrolCommand);
      setCopied(what);
    } catch {
      setCopied(null);
    }
  };
  return (
    <section className="flex flex-col gap-3 rounded-box border border-base-300 p-4" aria-label="Join token" data-testid="issued-token">
      <h3 className="font-semibold">Join token for {approved.name}</h3>
      <p className="text-sm text-base-content/70">
        Shown only once and valid until {when(approved.expiresAt)}. On the server, run this command and paste the token when it asks:
      </p>
      <code className="block overflow-x-auto rounded bg-base-200 px-2 py-1.5 text-xs whitespace-nowrap" data-testid="enrol-command">{approved.enrolCommand}</code>
      <textarea readOnly className="textarea textarea-sm w-full font-mono text-xs" rows={3} value={approved.token} aria-label="Join token" data-testid="issued-token-value" onFocus={(e) => e.currentTarget.select()} />
      {approved.warnings.map((w) => <p key={w} className="flex items-start gap-2 text-sm" role="note"><span className="status status-warning mt-1.5 shrink-0" aria-hidden="true" />{w}</p>)}
      <div className="flex flex-wrap gap-2">
        <button type="button" className="btn btn-sm btn-outline" onClick={() => void copy("token")}>{copied === "token" ? "Token copied" : "Copy token"}</button>
        <button type="button" className="btn btn-sm btn-outline" onClick={() => void copy("command")}>{copied === "command" ? "Command copied" : "Copy command"}</button>
        <button type="button" className="btn btn-sm btn-ghost" onClick={onDismiss}>Hide token</button>
      </div>
    </section>
  );
}

const RESULT: Record<string, { label: string; dot: string }> = {
  pass: { label: "Passed", dot: "status-success" },
  warn: { label: "Warning", dot: "status-warning" },
  fail: { label: "Failed", dot: "status-error" },
  "not-checked": { label: "Not checked", dot: "status-neutral" },
};

/** Runs the admission checks for a joined machine, then admits it when none failed. */
function AdmitDialog({ name, onClose, onAdmitted }: { name: string | null; onClose: () => void; onAdmitted: (name: string) => void }) {
  const [result, setResult] = useState<Admission | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [round, setRound] = useState(0);
  useEffect(() => {
    if (!name) return;
    let current = true;
    setResult(null);
    setError(null);
    client.request("machine_admission", { params: { name } }).then(
      (r) => { if (current) setResult(r); },
      (e: unknown) => { if (current) setError(messageOf(e)); },
    );
    return () => { current = false; };
  }, [name, round]);
  const admit = async () => {
    if (!name) return;
    setBusy(true);
    setError(null);
    try {
      await client.request("admit_machine", { params: { name } });
      onAdmitted(name);
    } catch (e) {
      setError(messageOf(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <FormDialog open={!!name} title={`Admit ${name ?? ""}?`} onClose={onClose} testId="admit-dialog" wide>
      <p className="text-sm text-base-content/80">Once admitted, Incus can place new instances on this server.</p>
      {!result && !error && <p className="text-sm text-base-content/60" role="status">Checking the server…</p>}
      {result && (
        <>
          <TableFrame>
            <thead><tr><th>Check</th><th>Result</th><th>Details</th></tr></thead>
            <tbody>
              {result.checks.map((c) => (
                <tr key={c.id} data-testid="admission-check">
                  <td className="font-medium sm:whitespace-nowrap">{c.title}</td>
                  <td className="sm:whitespace-nowrap"><span className={`status ${RESULT[c.status]?.dot ?? "status-neutral"} mr-2`} aria-hidden="true" />{RESULT[c.status]?.label ?? c.status}</td>
                  <td className="text-sm">{sentence(c.detail)}</td>
                </tr>
              ))}
            </tbody>
          </TableFrame>
          {!result.admissible && <p role="alert" className="text-sm text-error" data-testid="admission-blocked">Fix the failed checks on the server, then check again.</p>}
        </>
      )}
      {error && <ErrorPanel message={error} testId="admit-error" />}
      <div className="modal-action mt-0">
        <button type="button" className="btn btn-sm btn-outline" onClick={onClose}>Cancel</button>
        {result && !result.admissible
          ? <button type="button" className="btn btn-sm btn-ghost" onClick={() => setRound((r) => r + 1)}>Check again</button>
          : <button type="button" className="btn btn-sm btn-primary" disabled={!result || busy} onClick={() => void admit()} data-testid="admit-start">{busy ? "Admitting…" : "Admit server"}</button>}
      </div>
    </FormDialog>
  );
}

function Actions({ m, busy, onApprove, onWithdraw, onAdmit, onRemove }: {
  m: Machine; busy: boolean; onApprove: () => void; onWithdraw: () => void; onAdmit: () => void; onRemove: () => void;
}) {
  const remove = <button type="button" className="btn btn-xs btn-ghost text-error" onClick={onRemove} aria-label={`Remove ${m.name}`}>Remove…</button>;
  switch (m.state) {
    case "registered":
    case "approval-expired":
    case "left":
      return <>{remove}<button type="button" className="btn btn-xs btn-outline" disabled={busy} onClick={onApprove} data-testid="approve-machine">{m.state === "registered" ? "Approve" : "Approve again"}</button></>;
    case "approved":
      return <>{remove}<button type="button" className="btn btn-xs btn-ghost" disabled={busy} onClick={onWithdraw} data-testid="withdraw-machine">Withdraw approval</button></>;
    case "held":
      return <button type="button" className="btn btn-xs btn-outline" onClick={onAdmit} data-testid="admit-machine">Check and admit…</button>;
    case "eligible":
      return null;
  }
}

/**
 * Servers being added to the cluster: register, approve (join token, shown once), the server
 * joins with the enrolment script and is held from placement, then checks and admission.
 */
export function MachinesSection({ onAuth, onChanged }: { onAuth: (m: string) => void; onChanged: () => void }) {
  const [list, reload] = useLoad<MachineList>(() => client.get("list_machines"), onAuth);
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [approved, setApproved] = useState<MachineApproved | null>(null);
  const [toAdmit, setToAdmit] = useState<string | null>(null);
  const [toRemove, setToRemove] = useState<string | null>(null);
  const nameError = name && !NAME.test(name) ? "Use 1–63 letters, digits and hyphens, starting with a letter." : null;

  const act = async (work: () => Promise<unknown>) => {
    setError(null);
    setNotice(null);
    setBusy(true);
    try {
      await work();
      reload();
      onChanged();
    } catch (e) {
      setError(messageOf(e));
    } finally {
      setBusy(false);
    }
  };
  const register = () => act(async () => {
    await client.request("register_machine", { body: { name, description } });
    setName("");
    setDescription("");
  });
  const approve = (n: string) => act(async () => setApproved(await client.request("approve_machine", { params: { name: n } })));
  const withdraw = (n: string) => act(async () => {
    await client.request("withdraw_machine_approval", { params: { name: n } });
    if (approved?.name === n) setApproved(null);
  });
  const remove = (n: string) => act(async () => {
    await client.request("remove_machine", { params: { name: n } });
    if (approved?.name === n) setApproved(null);
  });

  const wide = useWide();
  const actions = (m: Machine) => (
    <Actions m={m} busy={busy} onApprove={() => void approve(m.name)} onWithdraw={() => void withdraw(m.name)} onAdmit={() => setToAdmit(m.name)} onRemove={() => setToRemove(m.name)} />
  );

  return (
    <Section title="Machines" state={list} testId="machines" onRetry={reload}>
      {(l) => (
        <>
          <SectionHeader title="Machines" count={l.items.length} source={l.source} />
          <p className="text-sm text-base-content/70">New servers run nothing until you admit them.</p>
          <form className="flex flex-wrap items-start gap-2" aria-label="Add a machine" onSubmit={(e) => { e.preventDefault(); void register(); }}>
            <label className="flex flex-col gap-1 text-sm">
              <span className="sr-only">Server name</span>
              <input className="input input-sm w-56 font-mono" placeholder="server-04" value={name} onChange={(e) => setName(e.target.value)} autoComplete="off" spellCheck={false} aria-invalid={!!nameError} aria-describedby="machine-name-hint" data-testid="machine-name" />
              <span id="machine-name-hint" className={`text-xs ${nameError ? "text-error" : "text-base-content/60"}`}>{nameError ?? "Its name in the cluster."}</span>
            </label>
            <label className="flex flex-col gap-1 text-sm">
              <span className="sr-only">Note</span>
              <input className="input input-sm w-56" placeholder="Rack 2, slot 7 (optional)" maxLength={200} value={description} onChange={(e) => setDescription(e.target.value)} autoComplete="off" data-testid="machine-description" />
            </label>
            <button type="submit" className="btn btn-sm btn-primary" disabled={busy || !name || !!nameError} data-testid="register-machine">Add machine</button>
          </form>
          {error && <ErrorPanel message={error} testId="machines-error" />}
          {notice && <p role="status" className="flex items-start gap-2 text-sm" data-testid="machines-notice"><span className="status status-success mt-1.5 shrink-0" aria-hidden="true" />{notice}</p>}
          {approved && <ApprovedToken approved={approved} onDismiss={() => setApproved(null)} />}
          {l.items.length === 0 ? <p className="text-sm text-base-content/70">No machines yet. Add one after installing its operating system and Incus.</p> : (
            <TableFrame>
              <thead><tr><th>Machine</th><th>State</th><th className="hidden sm:table-cell">Next step</th>{wide && <th><span className="sr-only">Actions</span></th>}</tr></thead>
              <tbody>
                {l.items.map((m) => (
                  <tr key={m.name} data-testid="machine-row">
                    <td className="font-medium">{m.name}{m.description && <span className="block text-xs font-normal text-base-content/60">{m.description}</span>}</td>
                    <td className="sm:whitespace-nowrap"><span data-testid="machine-state"><span className={`status ${STATES[m.state].dot} mr-2`} aria-hidden="true" />{STATES[m.state].label}</span><span className="mt-1 block text-xs whitespace-normal text-base-content/70 sm:hidden">{m.nextStep}{m.warnings.map((w) => <span key={w} className="mt-1 block text-warning-content" role="note">{sentence(w)}</span>)}</span>{!wide && <span className="mt-2 flex flex-wrap gap-1">{actions(m)}</span>}</td>
                    <td className="hidden text-sm sm:table-cell">
                      {m.nextStep}
                      {m.warnings.map((w) => <span key={w} className="mt-1 flex items-start gap-2" role="note"><span className="status status-warning mt-1.5 shrink-0" aria-hidden="true" />{sentence(w)}</span>)}
                    </td>
                    {wide && <td><div className="flex justify-end gap-1">{actions(m)}</div></td>}
                  </tr>
                ))}
              </tbody>
            </TableFrame>
          )}
          <AdmitDialog
            name={toAdmit}
            onClose={() => setToAdmit(null)}
            onAdmitted={(n) => { setToAdmit(null); setNotice(`${n} is in service: Incus can place instances on it.`); reload(); onChanged(); }}
          />
          <ConfirmDialog
            open={!!toRemove}
            title={`Remove ${toRemove ?? ""}?`}
            body={<p>Apelora forgets this machine, and its unused join token stops working. You can add it again later.</p>}
            action="Remove machine"
            onCancel={() => setToRemove(null)}
            onConfirm={() => { const n = toRemove; setToRemove(null); if (n) void remove(n); }}
          />
        </>
      )}
    </Section>
  );
}
