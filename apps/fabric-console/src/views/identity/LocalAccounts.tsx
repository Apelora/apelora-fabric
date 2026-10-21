import { useEffect, useId, useRef, useState } from "react";

import type { UserDto } from "@apelora/api-client";

import type { OneTimeSecret } from "../../components/SecretOnce";
import { ErrorPanel, FormDialog } from "../../components/ui";
import { client } from "../../lib/api";
import { fmtTime, useRun } from "../../lib/identity";

/** The issuer of every local account. */
export const LOCAL_ISSUER = "apelora:local";
export const isLocal = (u: UserDto) => u.issuer === LOCAL_ISSUER;

/** The link a person opens to choose their password; the token stays in the fragment. */
export const setupLink = (token: string) => `${window.location.origin}${window.location.pathname}#/set-password?token=${encodeURIComponent(token)}`;

/** The one-time setup link as the secret dialog shows it. */
export function setupSecret(who: string, token: string, expiresAt: string): OneTimeSecret {
  return {
    title: `Setup link for ${who}`,
    what: "setup link",
    value: setupLink(token),
    closeLabel: "I have sent it",
    note: <>Send it to {who} over a channel you trust. It works once, until {fmtTime(expiresAt)}.</>,
  };
}

const USERNAME = /^[a-z0-9._-]{2,64}$/;

/** Creates a local account; the person chooses their password with the setup link shown next. */
export function CreateLocalAccountDialog({ open, onAuth, onClose, onCreated }: {
  open: boolean; onAuth: (m: string) => void; onClose: () => void; onCreated: (user: UserDto, secret: OneTimeSecret) => void;
}) {
  const [username, setUsername] = useState("");
  const [displayName, setDisplayName] = useState("");
  const [email, setEmail] = useState("");
  const [invalid, setInvalid] = useState(false);
  const save = useRun(onAuth);
  const id = useId();
  const userRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!open) return;
    setUsername(""); setDisplayName(""); setEmail(""); setInvalid(false); save.setError(null);
  }, [open]); // eslint-disable-line react-hooks/exhaustive-deps

  const submit = (e: React.FormEvent) => {
    e.preventDefault();
    const name = username.trim().toLowerCase();
    if (!USERNAME.test(name)) { setInvalid(true); userRef.current?.focus(); return; }
    void save.run(async () => {
      const r = await client.request("create_local_account", { body: { username: name, displayName: displayName.trim() || null, email: email.trim() || null } });
      onCreated(r.user, setupSecret(r.user.displayName ?? name, r.setupToken, r.setupExpiresAt));
    });
  };

  return (
    <FormDialog open={open} title="Create a local account" onClose={onClose} testId="local-account-dialog">
      {open && <form className="flex flex-col gap-3" noValidate onSubmit={submit}>
        <p className="text-sm text-base-content/70">They choose their own password with a setup link you send them.</p>
        <div className="flex flex-col gap-1 text-sm">
          <label htmlFor={`${id}-u`}>Username</label>
          <input
            id={`${id}-u`} ref={userRef} className={`input input-sm w-full font-mono ${invalid ? "input-error" : ""}`} autoComplete="off" autoCapitalize="none" spellCheck={false} autoFocus
            aria-invalid={invalid || undefined} aria-describedby={`${id}-uh`}
            value={username} onChange={(e) => { setUsername(e.target.value); setInvalid(false); }} placeholder="alex.morgan" data-testid="local-username"
          />
          <span id={`${id}-uh`} className={`text-xs ${invalid ? "text-error" : "text-base-content/60"}`}>Lowercase letters, digits, dots, hyphens or underscores.</span>
        </div>
        <div className="grid gap-3 sm:grid-cols-2">
          <div className="flex flex-col gap-1 text-sm">
            <label htmlFor={`${id}-n`}>Display name (optional)</label>
            <input id={`${id}-n`} className="input input-sm w-full" autoComplete="off" maxLength={80} value={displayName} onChange={(e) => setDisplayName(e.target.value)} placeholder="Alex Morgan" data-testid="local-display-name" />
          </div>
          <div className="flex flex-col gap-1 text-sm">
            <label htmlFor={`${id}-e`}>E-mail (optional)</label>
            <input id={`${id}-e`} type="email" className="input input-sm w-full" autoComplete="off" spellCheck={false} maxLength={254} value={email} onChange={(e) => setEmail(e.target.value)} placeholder="alex@example.org" data-testid="local-email" />
          </div>
        </div>
        {save.error && <ErrorPanel message={save.error} testId="local-account-error" />}
        <div className="modal-action mt-0">
          <button type="button" className="btn btn-sm btn-outline" onClick={onClose}>Cancel</button>
          <button type="submit" className="btn btn-sm btn-primary" disabled={save.busy} aria-busy={save.busy || undefined} data-testid="local-account-create">
            {save.busy && <span className="loading loading-spinner loading-xs" aria-hidden="true" />}
            Create account
          </button>
        </div>
      </form>}
    </FormDialog>
  );
}
