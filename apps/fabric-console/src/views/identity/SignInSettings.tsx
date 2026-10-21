import { useEffect, useId, useState } from "react";

import type { ProviderCheck, ProviderInput, ProviderView, SignInSettings } from "@apelora/api-client";

import { Badge, ConfirmDialog, ErrorPanel, FormDialog, Notice, Section, SectionHeader, TableFrame } from "../../components/ui";
import { client, useLoad } from "../../lib/api";
import { useRun } from "../../lib/identity";
import { DirectoriesSection } from "./Directories";

const list = (s: string) => s.split(/[\s,]+/).map((x) => x.trim()).filter(Boolean);
const joined = (v: string[] | undefined) => (v ?? []).join(", ");

/** Where the Console and the Studio come back to after the provider signed someone in. */
const consoleRedirect = () => `${window.location.origin}${window.location.pathname}`;

function ProviderDialog({ open, editing, onAuth, onClose, onSaved }: { open: boolean; editing: ProviderView | null; onAuth: (m: string) => void; onClose: () => void; onSaved: () => void }) {
  const [name, setName] = useState("");
  const [issuer, setIssuer] = useState("");
  const [audiences, setAudiences] = useState("");
  const [clientId, setClientId] = useState("");
  const [scopes, setScopes] = useState("openid profile email");
  const [groupsClaim, setGroupsClaim] = useState("groups");
  const [usernameClaim, setUsernameClaim] = useState("preferred_username");
  const [mfaAmr, setMfaAmr] = useState("mfa, otp, hwk");
  const [mfaAcr, setMfaAcr] = useState("");
  const [parties, setParties] = useState("");
  const [enabled, setEnabled] = useState(true);
  const [checked, setChecked] = useState<ProviderCheck | null>(null);
  const save = useRun(onAuth);
  const check = useRun(onAuth);
  const base = useId();
  const ids = Array.from({ length: 10 }, (_, i) => `${base}-${i}`);

  useEffect(() => {
    if (!open) return;
    const e = editing;
    setName(e?.name ?? ""); setIssuer(e?.issuer ?? ""); setAudiences(joined(e?.audiences)); setClientId(e?.clientId ?? "");
    setScopes(e ? e.scopes.join(" ") : "openid profile email"); setGroupsClaim(e?.groupsClaim ?? "groups");
    setUsernameClaim(e?.usernameClaim ?? "preferred_username"); setMfaAmr(e ? joined(e.mfaAmr) : "mfa, otp, hwk");
    setMfaAcr(joined(e?.mfaAcr)); setParties(joined(e?.authorizedParties)); setEnabled(e?.enabled ?? true);
    setChecked(null); save.setError(null); check.setError(null);
  }, [open, editing]); // eslint-disable-line react-hooks/exhaustive-deps

  const issuerOk = /^(https:\/\/|http:\/\/(127\.0\.0\.1|localhost)(:\d+)?)/.test(issuer.trim());
  const ok = !!name.trim() && issuerOk && list(audiences).length > 0 && !save.busy;
  const body = (): ProviderInput => ({
    name: name.trim(), issuer: issuer.trim(), audiences: list(audiences), clientId: clientId.trim() || null,
    scopes: list(scopes), groupsClaim: groupsClaim.trim() || null, usernameClaim: usernameClaim.trim() || null,
    mfaAmr: list(mfaAmr), mfaAcr: list(mfaAcr), authorizedParties: list(parties), enabled,
  });
  const submit = () => void save.run(async () => {
    if (editing?.id) await client.request("update_identity_provider", { params: { id: editing.id }, body: body() });
    else await client.request("create_identity_provider", { body: body() });
    onSaved();
  });
  const runCheck = () => void check.run(async () => {
    setChecked(await client.request("check_identity_provider", { body: { issuer: issuer.trim() } }));
  });

  return (
    <FormDialog open={open} title={editing ? `Edit ${editing.name}` : "Add an identity provider"} onClose={onClose} testId="provider-dialog" wide>
      <form className="flex flex-col gap-3" onSubmit={(e) => { e.preventDefault(); if (ok) submit(); }}>
        <p className="text-sm text-base-content/70">Any OpenID Connect provider works. Roles are still given in Apelora.</p>
        <div className="grid gap-3 sm:grid-cols-2">
          <div className="flex flex-col gap-1 text-sm">
            <label htmlFor={ids[0]}>Name on the sign-in button</label>
            <input id={ids[0]} className="input input-sm w-full" value={name} onChange={(e) => setName(e.target.value)} maxLength={60} placeholder="Example Corp" data-testid="provider-name" />
          </div>
          <div className="flex flex-col gap-1 text-sm">
            <label htmlFor={ids[1]}>Issuer URL</label>
            <div className="flex gap-2 max-sm:flex-col">
              <input id={ids[1]} className="input input-sm w-full font-mono" value={issuer} onChange={(e) => { setIssuer(e.target.value); setChecked(null); }} placeholder="https://id.example.com/realms/main" spellCheck={false} aria-invalid={!!issuer && !issuerOk} data-testid="provider-issuer" />
              <button type="button" className="btn btn-sm btn-outline shrink-0" disabled={!issuerOk || check.busy} onClick={runCheck} data-testid="provider-check">{check.busy ? "Checking…" : "Check connection"}</button>
            </div>
            {!!issuer && !issuerOk && <span className="text-xs text-error">Use https:// (plain http only for a provider on this computer).</span>}
          </div>
        </div>
        {checked && <Notice tone="ok" testId="provider-check-ok" title="The provider answers">It publishes {checked.keys} signing {checked.keys === 1 ? "key" : "keys"}; sign-in goes to {new URL(checked.authorizationEndpoint).host}.</Notice>}
        {check.error && <ErrorPanel message={`The provider did not answer: ${check.error}`} testId="provider-check-error" />}
        <div className="grid gap-3 sm:grid-cols-2">
          <div className="flex flex-col gap-1 text-sm">
            <label htmlFor={ids[2]}>Audience</label>
            <input id={ids[2]} className="input input-sm w-full font-mono" value={audiences} onChange={(e) => setAudiences(e.target.value)} placeholder="apelora-api" spellCheck={false} data-testid="provider-audiences" />
            <span className="text-xs text-base-content/60">Tokens must be issued for this name. Separate several with commas.</span>
          </div>
          <div className="flex flex-col gap-1 text-sm">
            <label htmlFor={ids[3]}>Browser client ID (optional)</label>
            <input id={ids[3]} className="input input-sm w-full font-mono" value={clientId} onChange={(e) => setClientId(e.target.value)} placeholder="apelora-console" spellCheck={false} data-testid="provider-client" />
            <span className="text-xs text-base-content/60">A public client that may return to <span className="font-mono">{consoleRedirect()}</span> and the Studio. Without it, no sign-in button.</span>
          </div>
        </div>
        <details className="rounded-box border border-base-300 px-3 py-2 text-sm" data-testid="provider-advanced">
          <summary className="cursor-pointer select-none">Claims and multi-factor rules</summary>
          <div className="mt-3 grid gap-3 sm:grid-cols-2">
            <div className="flex flex-col gap-1">
              <label htmlFor={ids[4]}>Scopes</label>
              <input id={ids[4]} className="input input-sm w-full font-mono" value={scopes} onChange={(e) => setScopes(e.target.value)} spellCheck={false} data-testid="provider-scopes" />
            </div>
            <div className="flex flex-col gap-1">
              <label htmlFor={ids[5]}>Groups claim</label>
              <input id={ids[5]} className="input input-sm w-full font-mono" value={groupsClaim} onChange={(e) => setGroupsClaim(e.target.value)} spellCheck={false} />
            </div>
            <div className="flex flex-col gap-1">
              <label htmlFor={ids[6]}>Username claim</label>
              <input id={ids[6]} className="input input-sm w-full font-mono" value={usernameClaim} onChange={(e) => setUsernameClaim(e.target.value)} spellCheck={false} />
            </div>
            <div className="flex flex-col gap-1">
              <label htmlFor={ids[7]}>Methods that count as multi-factor (amr)</label>
              <input id={ids[7]} className="input input-sm w-full font-mono" value={mfaAmr} onChange={(e) => setMfaAmr(e.target.value)} spellCheck={false} />
            </div>
            <div className="flex flex-col gap-1">
              <label htmlFor={ids[8]}>Assurance levels that count as multi-factor (acr)</label>
              <input id={ids[8]} className="input input-sm w-full font-mono" value={mfaAcr} onChange={(e) => setMfaAcr(e.target.value)} spellCheck={false} />
            </div>
            <div className="flex flex-col gap-1">
              <label htmlFor={ids[9]}>Clients a token must be issued to (azp)</label>
              <input id={ids[9]} className="input input-sm w-full font-mono" value={parties} onChange={(e) => setParties(e.target.value)} spellCheck={false} />
            </div>
          </div>
        </details>
        <label className="flex items-center gap-2 text-sm">
          <input type="checkbox" className="checkbox checkbox-sm" checked={enabled} onChange={(e) => setEnabled(e.target.checked)} data-testid="provider-enabled" />
          Accept sign-ins from this provider
        </label>
        {save.error && <ErrorPanel message={save.error} testId="provider-save-error" />}
        <div className="modal-action mt-0">
          <button type="button" className="btn btn-sm btn-outline" onClick={onClose}>Cancel</button>
          <button type="submit" className="btn btn-sm btn-primary" disabled={!ok} data-testid="provider-save">{save.busy ? "Saving…" : editing ? "Save changes" : "Add provider"}</button>
        </div>
      </form>
    </FormDialog>
  );
}

/**
 * Identity & Access → Sign-in: the identity providers people sign in with, and whether API
 * tokens are accepted. Changes apply at once; the server refuses any that would lock everyone out.
 */
export function SignInSettingsView({ onAuth }: { onAuth: (m: string) => void }) {
  const [state, reload] = useLoad<SignInSettings>(() => client.get("sign_in_settings"), onAuth);
  const [dialog, setDialog] = useState<{ editing: ProviderView | null } | null>(null);
  const [toDelete, setToDelete] = useState<ProviderView | null>(null);
  const [tokensOff, setTokensOff] = useState(false);
  const action = useRun(onAuth);

  const setTokens = (enabled: boolean) => void action.run(async () => {
    await client.request("set_api_tokens", { body: { enabled } });
    reload();
  });
  const toggle = (p: ProviderView) => void action.run(async () => {
    if (!p.id) return;
    await client.request("update_identity_provider", { params: { id: p.id }, body: {
      name: p.name, issuer: p.issuer, audiences: p.audiences, clientId: p.clientId ?? null, scopes: p.scopes, groupsClaim: p.groupsClaim,
      usernameClaim: p.usernameClaim, mfaAmr: p.mfaAmr, mfaAcr: p.mfaAcr, authorizedParties: p.authorizedParties, enabled: !p.enabled } });
    reload();
  });
  const remove = (p: ProviderView) => void action.run(async () => {
    if (!p.id) return;
    await client.request("delete_identity_provider", { params: { id: p.id }, body: { confirm: p.name } });
    reload();
  });

  return (
    <Section title="Sign-in" level={1} state={state} testId="sign-in-settings" onRetry={reload}>
      {(s) => (
        <>
          <SectionHeader title="Sign-in" level={1} count={s.providers.length}>
            <button type="button" className="btn btn-sm btn-primary" onClick={() => setDialog({ editing: null })} data-testid="provider-add">Add identity provider</button>
          </SectionHeader>
          {action.error && <ErrorPanel message={action.error} testId="sign-in-settings-error" />}
          {s.providers.length === 0 ? (
            <p className="text-sm text-base-content/70" data-testid="no-providers">No identity provider yet. Add one so people sign in with their organisation's account.</p>
          ) : (
            <TableFrame>
              <thead><tr><th>Provider</th><th className="max-sm:hidden">Issuer</th><th>Status</th><th><span className="sr-only">Actions</span></th></tr></thead>
              <tbody>
                {s.providers.map((p) => (
                  <tr key={p.id ?? `env-${p.issuer}`} data-testid="provider-row">
                    <td className="font-medium">{p.name}<span className="block text-xs font-normal text-base-content/60">{p.clientId ? "Sign-in button shown" : "Tokens only, no button"}{p.source === "environment" ? "; set by environment variables" : ""}</span></td>
                    <td className="max-sm:hidden font-mono text-xs [overflow-wrap:anywhere]">{p.issuer}</td>
                    <td className="text-sm">
                      {!p.enabled ? <Badge tone="neutral">Off</Badge> : p.reachable ? <Badge tone="ok">Answers</Badge> : <Badge tone="error" title={p.error ?? undefined}>Not reachable</Badge>}
                      {p.enabled && !p.reachable && p.error && <span className="mt-1 block text-xs text-error">{p.error}</span>}
                    </td>
                    <td className="text-right">
                      {p.source === "console" && (
                        <span className="inline-flex flex-wrap justify-end gap-1 sm:flex-nowrap">
                          <button type="button" className="btn btn-xs btn-ghost" aria-label={`Edit ${p.name}`} onClick={() => setDialog({ editing: p })}>Edit…</button>
                          <button type="button" className="btn btn-xs btn-ghost" aria-label={`${p.enabled ? "Turn off" : "Turn on"} ${p.name}`} disabled={action.busy} onClick={() => toggle(p)}>{p.enabled ? "Turn off" : "Turn on"}</button>
                          <button type="button" className="btn btn-xs btn-ghost text-error" aria-label={`Remove ${p.name}`} onClick={() => setToDelete(p)}>Remove…</button>
                        </span>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </TableFrame>
          )}

          <DirectoriesSection directories={s.directories ?? []} onAuth={onAuth} reload={reload} />

          <section className="flex flex-col gap-2 rounded-box border border-base-300 p-4" data-testid="api-tokens-setting">
            <h2 className="text-sm font-semibold">API tokens</h2>
            <p className="text-sm text-base-content/70">
              {s.apiTokens.lockedByEnvironment
                ? "Turned off by the control plane's environment (APELORA_LEGACY_TOKENS=disabled)."
                : s.apiTokens.enabled
                  ? "Accepted. Once people sign in with a provider, turn them off; automation uses a service identity."
                  : "Turned off. People sign in with a provider; automation uses a service identity."}
            </p>
            {!s.apiTokens.lockedByEnvironment && (
              <button type="button" className="btn btn-sm btn-outline self-start" disabled={action.busy} onClick={() => (s.apiTokens.enabled ? setTokensOff(true) : setTokens(true))} data-testid="api-tokens-toggle">
                {s.apiTokens.enabled ? "Turn off API tokens…" : "Turn on API tokens"}
              </button>
            )}
          </section>

          <ProviderDialog open={!!dialog} editing={dialog?.editing ?? null} onAuth={onAuth} onClose={() => setDialog(null)} onSaved={() => { setDialog(null); reload(); }} />
          <ConfirmDialog
            open={!!toDelete}
            title={`Remove ${toDelete?.name ?? ""}?`}
            body={<p>People can no longer sign in with this provider. Their accounts and roles in Apelora stay.</p>}
            action="Remove provider"
            typeToConfirm={toDelete?.name}
            onCancel={() => setToDelete(null)}
            onConfirm={() => { const p = toDelete; setToDelete(null); if (p) remove(p); }}
          />
          <ConfirmDialog
            open={tokensOff}
            title="Turn off API tokens?"
            body={<p>Every API token stops working at once. Only someone signed in with a provider can do this.</p>}
            action="Turn off API tokens"
            onCancel={() => setTokensOff(false)}
            onConfirm={() => { setTokensOff(false); setTokens(false); }}
          />
        </>
      )}
    </Section>
  );
}
