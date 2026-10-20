import { useId, useState } from "react";

import type { RoleDto } from "@apelora/api-client";

import { ErrorPanel, FormDialog, Notice } from "../../components/ui";
import { client, useLoad } from "../../lib/api";
import { personName, useRun } from "../../lib/identity";
import { PersonLookup } from "./PersonLookup";

const MIN_REASON = 10;

function EmergencyForm({ onAuth, onClose, onGranted }: { onAuth: (m: string) => void; onClose: () => void; onGranted: (message: string) => void }) {
  const [roles] = useLoad<RoleDto[]>(() => client.get("roles").then((r) => r.items.filter((x) => x.builtin && x.scopes.includes("platform"))), onAuth);
  const [person, setPerson] = useState<{ id: string; label: string } | null>(null);
  const [roleKey, setRoleKey] = useState("");
  const [reason, setReason] = useState("");
  const [minutes, setMinutes] = useState(60);
  const [understood, setUnderstood] = useState(false);
  const { busy, error, run } = useRun(onAuth);
  const roleId = useId();
  const reasonId = useId();
  const minutesId = useId();
  const reasonOk = reason.trim().length >= MIN_REASON;
  const minutesOk = Number.isInteger(minutes) && minutes >= 5 && minutes <= 240;
  const role = roles.kind === "ok" ? roles.data.find((r) => r.reference.type === "builtin" && r.reference.key === roleKey) : undefined;
  const ready = !!person && !!role && reasonOk && minutesOk && understood && !busy;
  const submit = () => {
    if (!person || !role) return;
    void run(async () => {
      await client.request("grant_emergency_access", { body: { userId: person.id, role: roleKey, reason: reason.trim(), minutes } });
      onGranted(`Emergency access granted to ${person.label} as ${role.name} for ${minutes} minutes. It is recorded in the audit history.`);
    });
  };
  return (
    <div className="flex flex-col gap-4">
      <Notice tone="warn" title="Emergency access is audited and time-limited" testId="emergency-warning">
        This gives a person a platform role at once, outside the normal review. It ends by itself after the time you choose, every request made with it is recorded, and your reason is kept in the audit history. You cannot grant it to yourself.
      </Notice>

      {person ? (
        <div className="flex flex-wrap items-center gap-2 text-sm" data-testid="emergency-person">
          <span>Person: <strong>{person.label}</strong></span>
          <button type="button" className="btn btn-xs btn-ghost" onClick={() => setPerson(null)}>Change</button>
        </div>
      ) : (
        <PersonLookup onAuth={onAuth} label="Find the person" actionLabel="Choose" testId="emergency-lookup" onPick={(u) => setPerson({ id: u.id, label: personName(u) })} />
      )}

      <div className="flex flex-col gap-1 text-sm">
        <label htmlFor={roleId}>Platform role</label>
        <select id={roleId} className="select select-sm w-full" value={roleKey} onChange={(e) => setRoleKey(e.target.value)} disabled={roles.kind !== "ok"} data-testid="emergency-role">
          <option value="">{roles.kind === "loading" ? "Loading…" : "Choose a role…"}</option>
          {roles.kind === "ok" && roles.data.map((r) => r.reference.type === "builtin" && <option key={r.id} value={r.reference.key}>{r.name}</option>)}
        </select>
        {roles.kind === "error" && <ErrorPanel message={roles.message} />}
        {role && <span className="text-xs text-base-content/60">{role.description}</span>}
      </div>

      <div className="flex flex-col gap-1 text-sm">
        <label htmlFor={reasonId}>Reason (at least {MIN_REASON} characters)</label>
        <textarea id={reasonId} className="textarea textarea-sm w-full" rows={3} value={reason} onChange={(e) => setReason(e.target.value)} maxLength={300} aria-describedby={`${reasonId}-count`} data-testid="emergency-reason" />
        <span id={`${reasonId}-count`} className={`text-xs ${reasonOk || !reason ? "text-base-content/60" : "text-error"}`}>{reason.trim().length} / {MIN_REASON} characters minimum</span>
      </div>

      <div className="flex flex-col gap-1 text-sm">
        <label htmlFor={minutesId}>Lasts (minutes, 5–240)</label>
        <input id={minutesId} type="number" min={5} max={240} step={1} className="input input-sm w-32" value={minutes} onChange={(e) => setMinutes(Number(e.target.value))} aria-invalid={!minutesOk} data-testid="emergency-minutes" />
        {!minutesOk && <span className="text-xs text-error" role="alert">Emergency access lasts 5 to 240 minutes.</span>}
      </div>

      <label className="flex items-start gap-2 text-sm">
        <input type="checkbox" className="checkbox checkbox-xs mt-0.5" checked={understood} onChange={(e) => setUnderstood(e.target.checked)} data-testid="emergency-ack" />
        I understand this is recorded in the audit history and ends automatically.
      </label>

      {error && <ErrorPanel message={error} testId="emergency-error" />}
      <div className="modal-action mt-0">
        <button type="button" className="btn btn-sm btn-outline" onClick={onClose}>Cancel</button>
        <button type="button" className="btn btn-sm btn-primary" disabled={!ready} onClick={submit} data-testid="emergency-submit">{busy ? "Granting…" : "Grant emergency access"}</button>
      </div>
    </div>
  );
}

/** Break-glass: a platform role for a short time, with a recorded reason. For platform administrators. */
export function EmergencyAccessDialog({ open, onAuth, onClose, onGranted }: { open: boolean; onAuth: (m: string) => void; onClose: () => void; onGranted: (message: string) => void }) {
  return (
    <FormDialog open={open} title="Emergency access" onClose={onClose} testId="emergency-dialog">
      {open && <EmergencyForm onAuth={onAuth} onClose={onClose} onGranted={onGranted} />}
    </FormDialog>
  );
}
