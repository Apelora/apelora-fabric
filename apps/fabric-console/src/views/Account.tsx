import { useEffect, useId, useRef, useState } from "react";

import { ApiRequestError, type WhoAmI } from "@apelora/api-client";

import { FormDialog, Notice } from "../components/ui";
import { client, messageOf } from "../lib/api";
import { PASSWORD_RULE } from "./SignIn";
import { ACCOUNT_PAGES } from "./credentials/pages";

const length = (s: string) => Array.from(s).length;

/** Changes the signed-in person's own password (local accounts). Other sessions of theirs end. */
export function ChangePasswordDialog({ open, me, onClose, onAuth }: { open: boolean; me: WhoAmI; onClose: () => void; onAuth: (m: string) => void }) {
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const [again, setAgain] = useState("");
  const [errors, setErrors] = useState<{ current?: string; next?: string; again?: string }>({});
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [done, setDone] = useState(false);
  const refs = { current: useRef<HTMLInputElement>(null), next: useRef<HTMLInputElement>(null), again: useRef<HTMLInputElement>(null) };
  const id = useId();

  useEffect(() => {
    if (!open) return;
    setCurrent(""); setNext(""); setAgain(""); setErrors({}); setError(null); setDone(false);
  }, [open]);

  const fail = (field: "current" | "next" | "again", message: string) => { setErrors({ [field]: message }); refs[field].current?.focus(); };

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    setError(null);
    if (!current) return fail("current", "Enter your current password.");
    const n = length(next);
    if (n < 12 || n > 128) return fail("next", n < 12 ? `This has ${n} ${n === 1 ? "character" : "characters"}. Use at least 12.` : "Use at most 128 characters.");
    if (next !== again) return fail("again", "The two new passwords are different. Type the same password twice.");
    setErrors({});
    setBusy(true);
    try {
      await client.request("change_password", { body: { current, new: next } });
      setDone(true);
    } catch (err) {
      if (err instanceof ApiRequestError && err.unauthenticated) { onAuth(err.message); return; }
      if (err instanceof ApiRequestError && err.code === "wrong_password") fail("current", err.message);
      else if (err instanceof ApiRequestError && err.code === "weak_password") fail("next", err.message);
      else setError(messageOf(err));
    } finally {
      setBusy(false);
    }
  };

  const field = (key: "current" | "next" | "again", label: string, value: string, set: (v: string) => void, autoComplete: string, help?: string) => (
    <div className="flex flex-col gap-1 text-sm">
      <label htmlFor={`${id}-${key}`}>{label}</label>
      <input
        id={`${id}-${key}`} ref={refs[key]} type="password" autoComplete={autoComplete} autoFocus={key === "current"}
        className={`input input-sm w-full ${errors[key] ? "input-error" : ""}`}
        aria-invalid={!!errors[key] || undefined} aria-describedby={errors[key] || help ? `${id}-${key}-help` : undefined}
        value={value} onChange={(e) => { set(e.target.value); setErrors((x) => ({ ...x, [key]: undefined })); }}
        data-testid={`password-${key}`}
      />
      {(errors[key] || help) && <span id={`${id}-${key}-help`} className={`text-xs ${errors[key] ? "text-error" : "text-base-content/60"}`} data-testid={`password-${key}-help`}>{errors[key] ?? help}</span>}
    </div>
  );

  return (
    <FormDialog open={open} title="Change your password" onClose={onClose} testId="change-password-dialog">
      {!open ? null : done ? (
        <div className="flex flex-col gap-3">
          <Notice tone="ok" testId="change-password-done" title="Password changed">You stay signed in here and were signed out everywhere else.</Notice>
          <div className="modal-action mt-0">
            <button type="button" className="btn btn-sm btn-primary" onClick={onClose}>Close</button>
          </div>
        </div>
      ) : (
        <form className="flex flex-col gap-3" noValidate onSubmit={(e) => void submit(e)}>
          {/* Lets a password manager file the new password under the right account. */}
          <input type="text" name="username" autoComplete="username" value={me.name} readOnly hidden />
          {field("current", "Current password", current, setCurrent, "current-password")}
          {field("next", "New password", next, setNext, "new-password", PASSWORD_RULE)}
          {field("again", "New password again", again, setAgain, "new-password")}
          <p className="text-xs text-base-content/60">You will be signed out everywhere else.</p>
          {error && <Notice tone="error" testId="change-password-error">{error}</Notice>}
          <div className="modal-action mt-0">
            <button type="button" className="btn btn-sm btn-outline" onClick={onClose}>Cancel</button>
            <button type="submit" className="btn btn-sm btn-primary" disabled={busy} aria-busy={busy || undefined} data-testid="change-password-submit">
              {busy && <span className="loading loading-spinner loading-xs" aria-hidden="true" />}
              Change password
            </button>
          </div>
        </form>
      )}
    </FormDialog>
  );
}

/**
 * The person's name in the header, opening a small menu: Your tokens, Your sign-ins, Change
 * password (local accounts only) and Sign out.
 */
export function UserMenu({ me, onSignOut, onAuth }: { me: WhoAmI; onSignOut: () => void; onAuth: (m: string) => void }) {
  const [open, setOpen] = useState(false);
  const [changing, setChanging] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const button = useRef<HTMLButtonElement>(null);
  const menuId = useId();

  useEffect(() => {
    if (!open) return;
    const away = (e: MouseEvent) => { if (!root.current?.contains(e.target as Node)) setOpen(false); };
    const key = (e: KeyboardEvent) => { if (e.key === "Escape") { setOpen(false); button.current?.focus(); } };
    document.addEventListener("mousedown", away);
    document.addEventListener("keydown", key);
    root.current?.querySelector<HTMLButtonElement>("[role=menuitem]")?.focus();
    return () => { document.removeEventListener("mousedown", away); document.removeEventListener("keydown", key); };
  }, [open]);

  const label = me.role ? `${me.name} · ${me.role}` : me.name;
  return (
    <div ref={root} className="relative min-w-0">
      <button
        ref={button} type="button" className="btn btn-ghost btn-sm max-w-[14rem] gap-1 px-2 font-normal"
        aria-haspopup="menu" aria-expanded={open} aria-controls={menuId} onClick={() => setOpen((o) => !o)} data-testid="user-menu"
      >
        <span className="truncate text-xs text-base-content/70" data-testid="whoami">{label}</span>
        <svg viewBox="0 0 16 16" className="size-3 shrink-0 text-base-content/60" aria-hidden="true"><path d="M4 6l4 4 4-4" fill="none" stroke="currentColor" strokeWidth="1.5" /></svg>
      </button>
      {open && (
        <ul id={menuId} role="menu" aria-label="Your account" className="menu menu-sm absolute right-0 z-20 mt-1 w-52 rounded-box border border-base-300 bg-base-100 p-1 shadow-sm" data-testid="user-menu-list">
          {ACCOUNT_PAGES.map((p) => (
            <li role="none" key={p.id}><a role="menuitem" href={`#/${p.id}`} onClick={() => setOpen(false)} data-testid={`menu-${p.id.replace("account/", "")}`}>{p.label}</a></li>
          ))}
          {me.localAccount && (
            <li role="none"><button type="button" role="menuitem" onClick={() => { setOpen(false); setChanging(true); }} data-testid="menu-change-password">Change password</button></li>
          )}
          <li role="none"><button type="button" role="menuitem" onClick={() => { setOpen(false); onSignOut(); }} data-testid="menu-sign-out">Sign out</button></li>
        </ul>
      )}
      <ChangePasswordDialog open={changing} me={me} onAuth={onAuth} onClose={() => { setChanging(false); button.current?.focus(); }} />
    </div>
  );
}
