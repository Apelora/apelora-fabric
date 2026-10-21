import { DatabaseBackupsSection } from "./DatabaseBackups";
import { useState } from "react";

import type { Role, SystemInfo, TokenCreated, TokenInfo, TokenList, WhoAmI } from "@apelora/api-client";

import { ConfirmDialog, ErrorPanel, FormDialog, Section, SectionHeader, TableFrame } from "../components/ui";
import { client, messageOf, useLoad } from "../lib/api";
import { holdsPlatform } from "../lib/identity";
import { EffectiveSettingsSection } from "./EffectiveSettings";
import { ImageMirrorsSection } from "./ImageMirrors";
import { ProjectsSection } from "./Projects";
import { LicenceNoticesSection } from "./LicenceNotices";
import { MonitoringSection } from "./Monitoring";
import { SupportBundleSection } from "./SupportBundle";

const ROLES: { value: Role; label: string; what: string }[] = [
  { value: "auditor", label: "Auditor", what: "reads inventory and the audit log" },
  { value: "studio-designer", label: "Studio designer", what: "designs applications" },
  { value: "operator", label: "Operator", what: "deploys and operates instances" },
  { value: "fabric-admin", label: "Fabric admin", what: "everything, including tokens and membership" },
];

const when = (t: string | null | undefined) => (t ? new Date(t).toLocaleDateString() : "—");
const state = (t: TokenInfo) => (t.revokedAt ? "Revoked" : t.expiresAt && Date.parse(t.expiresAt) < Date.now() ? "Expired" : "Active");

function SystemSection({ onAuth }: { onAuth: (m: string) => void }) {
  const [info, reload] = useLoad<SystemInfo>(() => client.get("system"), onAuth);
  return (
    <Section title="System" state={info} testId="system" onRetry={reload}>
      {(s) => {
        // An older control plane does not report Incus here.
        const running = s.incus?.serverVersion;
        const pinned = s.incusPinned.replace(/^v/, "");
        return (
          <>
            <SectionHeader title="System" />
            <dl className="divide-y divide-base-300 rounded-box border border-base-300 text-sm">
              {[
                ["Apelora control plane", `${s.version} · API ${s.apiVersion}`],
                ["Incus running", running ?? (s.incus?.error ? `not reachable: ${s.incus.error}` : "not connected")],
                ["Incus pinned", `${pinned} (${s.incusSeries} LTS series)${running && running !== pinned ? ` · running ${running} differs` : ""}`],
                ["Tested with", s.incusTestedWith || "not recorded"],
              ].map(([k, v]) => (
                <div key={k} className="flex flex-col gap-1 px-4 py-3 sm:flex-row sm:justify-between sm:gap-6" data-testid="system-row">
                  <dt className="font-medium">{k}</dt>
                  <dd className="text-base-content/70 sm:max-w-[65%] sm:text-right">{v}</dd>
                </div>
              ))}
            </dl>
          </>
        );
      }}
    </Section>
  );
}

function CreateTokenDialog({ open, onClose, onCreated }: { open: boolean; onClose: () => void; onCreated: (t: TokenCreated) => void }) {
  const [name, setName] = useState("");
  const [role, setRole] = useState<Role>("auditor");
  const [days, setDays] = useState(90);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const submit = async () => {
    setBusy(true);
    setError(null);
    try {
      onCreated(await client.request("create_token", { body: { name: name.trim(), role, expiresInDays: days } }));
      setName("");
      onClose();
    } catch (e) {
      setError(messageOf(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <FormDialog open={open} title="Create an API token" onClose={onClose} testId="token-dialog">
      <form className="flex flex-col gap-3" onSubmit={(e) => { e.preventDefault(); void submit(); }}>
        <label className="flex flex-col gap-1 text-sm">
          <span>Name</span>
          <input className="input input-sm w-full" value={name} onChange={(e) => setName(e.target.value)} placeholder="ci-pipeline" maxLength={64} autoComplete="off" data-testid="token-name" />
          <span className="text-xs text-base-content/60">Who or what will use it.</span>
        </label>
        <label className="flex flex-col gap-1 text-sm">
          <span>Role</span>
          <select className="select select-sm w-full" value={role} onChange={(e) => setRole(e.target.value as Role)} data-testid="token-role">
            {ROLES.map((r) => <option key={r.value} value={r.value}>{r.label}: {r.what}</option>)}
          </select>
        </label>
        <label className="flex flex-col gap-1 text-sm">
          <span>Expires after (days)</span>
          <input type="number" min={1} max={365} className="input input-sm w-32" value={days} onChange={(e) => setDays(Number(e.target.value))} data-testid="token-days" />
        </label>
        {error && <ErrorPanel message={error} testId="token-error" />}
        <div className="modal-action mt-0">
          <button type="button" className="btn btn-sm btn-ghost" onClick={onClose}>Cancel</button>
          <button type="submit" className="btn btn-sm btn-primary" disabled={busy || !name.trim() || days < 1 || days > 365} data-testid="token-submit">{busy ? "Creating…" : "Create token"}</button>
        </div>
      </form>
    </FormDialog>
  );
}

function NewToken({ created, onHide }: { created: TokenCreated; onHide: () => void }) {
  const [copied, setCopied] = useState(false);
  return (
    <section className="flex flex-col gap-3 rounded-box border border-base-300 p-4" aria-label="New API token" data-testid="new-token">
      <h3 className="font-semibold">Token for {created.info.name}</h3>
      <p className="text-sm text-base-content/70">Shown only once. Valid until {when(created.info.expiresAt)}.</p>
      <input readOnly className="input input-sm w-full font-mono text-xs" value={created.token} aria-label="API token" data-testid="new-token-value" onFocus={(e) => e.currentTarget.select()} />
      <div className="flex gap-2">
        <button type="button" className="btn btn-sm btn-outline" onClick={() => void navigator.clipboard.writeText(created.token).then(() => setCopied(true), () => setCopied(false))}>{copied ? "Copied" : "Copy token"}</button>
        <button type="button" className="btn btn-sm btn-ghost" onClick={onHide}>Hide token</button>
      </div>
    </section>
  );
}

function TokensSection({ onAuth }: { onAuth: (m: string) => void }) {
  const [tokens, reload] = useLoad<TokenList>(() => client.get("tokens"), onAuth);
  const [creating, setCreating] = useState(false);
  const [created, setCreated] = useState<TokenCreated | null>(null);
  const [toRevoke, setToRevoke] = useState<TokenInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  const revoke = async (t: TokenInfo) => {
    setError(null);
    try {
      await client.request("revoke_token", { params: { id: t.id } });
      reload();
    } catch (e) {
      setError(messageOf(e));
    }
  };
  return (
    <>
      <Section title="API tokens" state={tokens} testId="tokens" onRetry={reload}>
        {(l) => (
          <>
            <SectionHeader title="API tokens" count={l.items.filter((t) => state(t) === "Active").length}>
              <button type="button" className="btn btn-sm btn-primary" onClick={() => setCreating(true)} data-testid="token-open">Create token</button>
            </SectionHeader>
            {created && <NewToken created={created} onHide={() => setCreated(null)} />}
            {error && <ErrorPanel message={error} />}
            <TableFrame>
              <thead><tr><th>Name</th><th>Role</th><th>State</th><th className="max-sm:hidden">Last used</th><th className="max-sm:hidden">Expires</th><th><span className="sr-only">Actions</span></th></tr></thead>
              <tbody>
                {l.items.map((t) => (
                  <tr key={t.id} data-testid="token-row" className={state(t) === "Active" ? undefined : "text-base-content/50"}>
                    <td className="font-medium">{t.name}</td>
                    <td>{ROLES.find((r) => r.value === t.role)?.label ?? t.role}</td>
                    <td>{state(t)}</td>
                    <td className="max-sm:hidden">{when(t.lastUsedAt)}</td>
                    <td className="max-sm:hidden">{when(t.expiresAt)}</td>
                    <td className="text-right">
                      {state(t) === "Active" && <button type="button" className="btn btn-xs btn-ghost text-error" aria-label={`Revoke token ${t.name}`} onClick={() => setToRevoke(t)}>Revoke</button>}
                    </td>
                  </tr>
                ))}
              </tbody>
            </TableFrame>
          </>
        )}
      </Section>
      <CreateTokenDialog open={creating} onClose={() => setCreating(false)} onCreated={(t) => { setCreated(t); reload(); }} />
      <ConfirmDialog
        open={!!toRevoke}
        title={`Revoke the token ${toRevoke?.name ?? ""}?`}
        body={<p>Anything using it is locked out at once, including this console if you signed in with it.</p>}
        action="Revoke token"
        onCancel={() => setToRevoke(null)}
        onConfirm={() => { const t = toRevoke; setToRevoke(null); if (t) void revoke(t); }}
      />
    </>
  );
}

export function AdministrationView({ me, onAuth }: { me: WhoAmI | null; onAuth: (m: string) => void }) {
  return (
    <div className="flex flex-col gap-8">
      {me?.permissions.includes("manage-tokens") && <TokensSection onAuth={onAuth} />}
      <SystemSection onAuth={onAuth} />
      {holdsPlatform(me, "cluster.view") && <DatabaseBackupsSection onAuth={onAuth} />}
      <ProjectsSection onAuth={onAuth} />
      <ImageMirrorsSection onAuth={onAuth} />
      <MonitoringSection onAuth={onAuth} />
      {holdsPlatform(me, "cluster.view") && <EffectiveSettingsSection onAuth={onAuth} />}
      {holdsPlatform(me, "support.bundle") && <SupportBundleSection onAuth={onAuth} />}
      <LicenceNoticesSection onAuth={onAuth} />
    </div>
  );
}
