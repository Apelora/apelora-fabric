import { useEffect, useRef, useState } from "react";

import type { CertificateStatus, ClusterMembersReport, ClusterSetting, ClusterSettingsReport, ClusterVersionComparison, ClusterVersionState, SystemInfo, UpgradeView, WhoAmI } from "@apelora/api-client";

import { Badge, ErrorPanel, FormDialog, Notice, Section, SectionHeader, TableFrame, sentence, type Tone } from "../../components/ui";
import { client, messageOf, useLoad } from "../../lib/api";
import { holdsPlatform } from "../../lib/identity";
import { ChangeSetting } from "./ChangeSetting";

const stop = (s: string) => `${sentence(s)}${/[.!?]$/.test(s) ? "" : "."}`;

// ---- Cluster settings ---------------------------------------------------------------------------

/**
 * Servers → Cluster settings. A value that is not set shows Incus's default, muted. People who may
 * change settings get a *Change…* button per setting: a review, another person's approval and the
 * change itself (legacy administrator tokens need no approval); everyone else reads.
 */
export function SettingsTab({ me, onAuth }: { me: WhoAmI | null; onAuth: (m: string) => void }) {
  const [settings, reload] = useLoad<ClusterSettingsReport>(() => client.get("cluster_settings"), onAuth);
  const [editing, setEditing] = useState<ClusterSetting | null>(null);
  const legacyAdmin = !!me && me.kind !== "user" && me.kind !== "service" && me.permissions.includes("manage-infrastructure");
  const canChange = holdsPlatform(me, "cluster.sensitive_change") || legacyAdmin;
  return (
    <Section title="Cluster settings" state={settings} testId="cluster-settings" onRetry={reload}>
      {(s) => (
        <>
          <SectionHeader title="Cluster settings" source={s.source} />
          <p className="text-sm text-base-content/70">
            {canChange
              ? "Each change is reviewed first, needs another person's approval, and is audited."
              : <>Read-only here. Changing them needs the cluster.sensitive_change permission, or <code className="rounded bg-base-200 px-1 py-0.5 text-xs">incus config set</code> on a member.</>}
          </p>
          <dl className="divide-y divide-base-300 rounded-box border border-base-300 text-sm">
            {s.items.map((i) => (
              <div key={i.key} className="flex flex-col gap-1 px-4 py-3 sm:flex-row sm:items-baseline sm:justify-between sm:gap-6" data-testid="cluster-setting" data-default={i.usesDefault}>
                <dt className="flex min-w-0 flex-col gap-0.5">
                  <span className="font-medium">{i.label}</span>
                  <span className="text-base-content/70">{i.meaning}</span>
                  <code className="text-xs text-base-content/60">{i.key}</code>
                </dt>
                <dd className="flex shrink-0 flex-col gap-2 tabular-nums sm:items-end sm:text-right">
                  {i.usesDefault
                    ? <span className="text-base-content/70">{i.default != null ? `Default · ${i.default}` : "Default (not reported)"}</span>
                    : (
                      <>
                        <span className="font-semibold">{i.value}</span>
                        {i.default != null && <span className="block text-xs text-base-content/60">default {i.default}</span>}
                      </>
                    )}
                  {canChange && <button type="button" className="btn btn-xs btn-outline self-start sm:self-end" onClick={() => setEditing(i)} aria-label={`Change ${i.label}`} data-testid="change-setting-open">Change…</button>}
                </dd>
              </div>
            ))}
          </dl>
          {editing && <ChangeSetting key={editing.key} setting={editing} needsApproval={me?.credential !== "api-token" && me?.credential !== "development"} onClose={() => setEditing(null)} onChanged={reload} onAuth={onAuth} />}
          <p className="text-xs text-base-content/60">Read with Apelora's unrestricted Incus identity, the only one Incus shows its configuration to.</p>
        </>
      )}
    </Section>
  );
}

// ---- Certificates -------------------------------------------------------------------------------

const CERTIFICATES: Record<string, { label: string; what: string }> = {
  deploy: { label: "Deploy identity", what: "Client certificate the control plane and the worker use (restricted to the deploy projects)" },
  admin: { label: "Admin identity", what: "Unrestricted client certificate, control plane only (join tokens, evacuation, cluster settings)" },
  server: { label: "Incus server (pinned)", what: "The cluster certificate Apelora trusts" },
};

const RUNBOOK = "docs/runbooks/certificates.md";

/** When a certificate expires, in words; expiring and expired ones point to the rotation runbook. */
export function expiry(c: CertificateStatus): string {
  const date = new Date(c.expiresAt).toLocaleDateString();
  if (c.state === "expired") return `Expired ${date}: rotate it (${RUNBOOK})`;
  const left = `${c.daysLeft} ${c.daysLeft === 1 ? "day" : "days"} left`;
  return c.state === "expiring" ? `${left}: rotate it soon (${RUNBOOK})` : `${left} · until ${date}`;
}

const CERT_TONE: Record<string, Tone> = { ok: "ok", expiring: "warn", expired: "error" };

/** Servers → Certificates: the certificates Apelora uses to reach Incus and when they expire. */
export function CertificatesTab({ onAuth }: { onAuth: (m: string) => void }) {
  const [info, reload] = useLoad<SystemInfo>(() => client.get("system"), onAuth);
  return (
    <Section title="Certificates" state={info} testId="certificates" onRetry={reload}>
      {(s) => (
        <>
          <SectionHeader title="Certificates" count={s.certificates.length} />
          {s.certificates.length === 0
            ? <p className="text-sm text-base-content/70">No certificates: the control plane uses mock data, not a real cluster.</p>
            : (
              <ul className="divide-y divide-base-300 rounded-box border border-base-300 text-sm">
                {s.certificates.map((c) => (
                  <li key={c.certificate} className="flex flex-col gap-1 px-4 py-3 sm:flex-row sm:items-baseline sm:justify-between sm:gap-6" data-testid="certificate-row">
                    <span className="flex min-w-0 flex-col gap-0.5">
                      <span className="font-medium">{CERTIFICATES[c.certificate]?.label ?? c.certificate}</span>
                      {CERTIFICATES[c.certificate] && <span className="text-base-content/70">{CERTIFICATES[c.certificate]!.what}</span>}
                    </span>
                    <span className="flex items-start gap-2 sm:max-w-[50%] sm:justify-end sm:text-right">
                      <span className={`status mt-1.5 shrink-0 ${c.state === "ok" ? "status-success" : c.state === "expiring" ? "status-warning" : "status-error"}`} aria-hidden="true" />
                      <span>{expiry(c)}</span>
                    </span>
                  </li>
                ))}
              </ul>
            )}
          {s.certificates.some((c) => CERT_TONE[c.state] !== "ok") && (
            <Notice tone="warn" testId="certificates-due">Rotate the certificates marked above before they expire: the runbook is {RUNBOOK}.</Notice>
          )}
        </>
      )}
    </Section>
  );
}

// ---- Upgrades -----------------------------------------------------------------------------------

const VERSION_STATE: Record<ClusterVersionState, { label: string; tone: Tone }> = {
  "matches-pin": { label: "Up to date", tone: "ok" },
  "behind-pin": { label: "Upgrade due", tone: "warn" },
  "ahead-of-pin": { label: "Newer than the pin", tone: "warn" },
  mixed: { label: "Mixed releases", tone: "warn" },
  unknown: { label: "Unknown", tone: "neutral" },
};

const AGAINST: Record<ClusterVersionComparison, string> = {
  same: "The pinned release",
  older: "Older than the pin",
  newer: "Newer than the pin",
  unknown: "Cannot compare",
};

const UPGRADE_RUNBOOK = "docs/runbooks/incus-rolling-upgrade.md";

/** Servers → Upgrades: version skew against the pinned release and the rolling procedure. Read-only. */
export function UpgradesTab({ me, onAuth }: { me: WhoAmI | null; onAuth: (m: string) => void }) {
  const [report, reload] = useLoad<ClusterMembersReport>(() => client.get("cluster_members"), onAuth);
  return (
    <Section title="Upgrades" state={report} testId="upgrades" onRetry={reload}>
      {(r) => {
        const v = r.versions;
        const s = VERSION_STATE[v.state];
        return (
          <div className="flex flex-col gap-6">
            <div className="flex flex-col gap-3">
              <SectionHeader title="Incus releases" source={r.source}>
                <Badge tone={s.tone} testId="upgrade-state">{s.label}</Badge>
              </SectionHeader>
              <p className="text-sm" data-testid="upgrade-summary">{v.summary}</p>
              <TableFrame>
                <thead><tr><th>Release</th><th>Compared with {v.pinned}</th><th>Members</th></tr></thead>
                <tbody>
                  {v.groups.map((g) => (
                    <tr key={g.version ?? "unknown"} data-testid="version-group">
                      <td className="font-medium tabular-nums">{g.version ?? "Not readable"}</td>
                      <td>{AGAINST[g.againstPin]}</td>
                      <td>{g.members.join(", ")}</td>
                    </tr>
                  ))}
                </tbody>
              </TableFrame>
              {v.ready && <p className="flex items-start gap-2 text-sm" data-testid="upgrade-ready"><span className="status status-success mt-1.5 shrink-0" aria-hidden="true" />Nothing blocks upgrading the next member.</p>}
              {v.blockers.map((b) => <Notice key={b} tone="error" testId="upgrade-blocker" title="Wait before upgrading">{stop(b)}</Notice>)}
              {v.notes.map((n) => <p key={n} className="flex items-start gap-2 text-sm" data-testid="upgrade-note"><span className="status status-info mt-1.5 shrink-0" aria-hidden="true" />{stop(n)}</p>)}
            </div>

            {r.upgrade && (holdsPlatform(me, "cluster.evacuate") || !!me?.permissions.includes("manage-infrastructure")) && (
              <UpgradeAssistant view={r.upgrade} ready={v.ready} onDone={reload} />
            )}

            <section className="flex flex-col gap-2" aria-labelledby="rolling-title" data-testid="rolling-procedure">
              <h2 id="rolling-title" className="font-semibold">Rolling upgrade, one member at a time</h2>
              <p className="text-sm text-base-content/70">Apelora does not install Incus. For each member:</p>
              <ol className="list-decimal space-y-1 pl-5 text-sm">
                <li>Check: nothing above blocks it, and Members shows a voter to spare.</li>
                <li>Evacuate it from Members, after reading the dry run.</li>
                <li>Upgrade Incus on that server and restart Incus.</li>
                <li>Wait until it is Online with the new release. <em>Blocked</em> means the release changes the database: upgrade the rest without waiting.</li>
                <li>Restore it. Upgrade the database leader last.</li>
              </ol>
              <p className="text-sm text-base-content/70">Full procedure: {UPGRADE_RUNBOOK}</p>
            </section>
          </div>
        );
      }}
    </Section>
  );
}

/** Focuses `ref` once its dialog is visible. The modal is `visibility: hidden` while it opens. */
function useFocusWhenShown(ref: { current: HTMLElement | null }, active: boolean) {
  useEffect(() => {
    if (!active) return;
    let frames = 30;
    let id = 0;
    const focus = () => {
      const el = ref.current;
      el?.focus();
      if (el && document.activeElement !== el && frames-- > 0) id = requestAnimationFrame(focus);
    };
    focus();
    return () => cancelAnimationFrame(id);
  }, [ref, active]);
}

const STEP: Record<string, string> = {
  start: "Start the upgrade",
  evacuate: "Evacuate this member",
  upgraded: "Record the package upgrade",
  restore: "Restore this member",
};

/**
 * The guided run. Apelora evacuates and restores. The package upgrade is done on the server;
 * recording it does not install anything.
 */
function UpgradeAssistant({ view, ready, onDone }: { view: UpgradeView; ready: boolean; onDone: () => void }) {
  const [open, setOpen] = useState<"step" | "abandon" | null>(null);
  const [typed, setTyped] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const field = useRef<HTMLInputElement>(null);
  useFocusWhenShown(field, open !== null);
  const word = view.next === "start" || open === "abandon" ? "upgrade" : (view.member ?? "");
  const label = STEP[view.next] ?? "Continue";

  const send = async (action: string, member: string, confirm: string) => {
    setBusy(true);
    setError(null);
    try {
      await client.request("upgrade_cluster", { body: { action, member, confirm } });
      setOpen(null);
      setTyped("");
      onDone();
    } catch (e) {
      setError(messageOf(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="flex flex-col gap-2" aria-labelledby="assistant-title" data-testid="upgrade-assistant">
      <h2 id="assistant-title" className="font-semibold">Upgrade assistant</h2>
      <p className="text-sm" data-testid="upgrade-assistant-summary">{view.summary}</p>
      {view.order.length > 0 && (
        <ol className="list-decimal space-y-1 pl-5 text-sm" data-testid="upgrade-order">
          {view.order.map((name) => <li key={name} className={name === view.member ? "font-medium" : undefined}>{name}{name === view.member ? " — this member" : ""}</li>)}
        </ol>
      )}
      <div className="flex flex-wrap gap-2">
        {view.next !== "none" && (
          <button type="button" className="btn btn-sm btn-primary" disabled={view.next === "start" && !ready} onClick={() => { setTyped(""); setError(null); setOpen("step"); }} data-testid="upgrade-next">{label}</button>
        )}
        {view.running && (
          <button type="button" className="btn btn-sm btn-ghost text-error" onClick={() => { setTyped(""); setError(null); setOpen("abandon"); }} data-testid="upgrade-abandon">Abandon the run</button>
        )}
      </div>
      <FormDialog open={open !== null} title={open === "abandon" ? "Abandon this upgrade?" : label} onClose={() => setOpen(null)} testId="upgrade-dialog">
        <p className="text-sm text-base-content/80">
          {open === "abandon"
            ? "The run is forgotten. Members that were evacuated stay evacuated until you restore them from Members."
            : view.next === "upgraded"
              ? "This records that you upgraded Incus on the server. Apelora does not install the package; it checks that the server now reports the new release."
              : view.summary}
        </p>
        <label className="flex flex-col gap-1 text-sm">
          Type <span className="font-mono">{word}</span> to confirm
          <input ref={field} className="input input-sm w-full font-mono" value={typed} onChange={(e) => setTyped(e.target.value)} autoComplete="off" spellCheck={false} data-testid="upgrade-confirm" />
        </label>
        {error && <ErrorPanel message={error} testId="upgrade-error" />}
        <div className="modal-action mt-0">
          <button type="button" className="btn btn-sm btn-outline" onClick={() => setOpen(null)}>Cancel</button>
          {open === "abandon"
            ? <button type="button" className="btn btn-sm btn-ghost text-error" disabled={busy || typed !== "upgrade"} onClick={() => void send("abandon", "", typed)} data-testid="upgrade-abandon-confirm">{busy ? "Abandoning…" : "Abandon the run"}</button>
            : <button type="button" className="btn btn-sm btn-primary" disabled={busy || typed !== word} onClick={() => void send(view.next, view.member ?? "", typed)} data-testid="upgrade-step-confirm">{busy ? "Working…" : label}</button>}
        </div>
      </FormDialog>
    </section>
  );
}
