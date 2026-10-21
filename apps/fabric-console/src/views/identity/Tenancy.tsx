import { useEffect, useId, useRef, useState, type ReactNode } from "react";

import type {
  QuotaChange, QuotaLimits, QuotaScopeKind, QuotaUsage, QuotaView, TenancyEnvironment, TenancyFinding, TenancyIncusState,
  TenancyOrganization, TenancyOverview, TenancyProject, TenancyProjectUse, TenancyResource,
} from "@apelora/api-client";
import { ApiRequestError } from "@apelora/api-client";

import { Badge, ErrorPanel, FormDialog, Notice, Section, SectionHeader, sentence, type Tone } from "../../components/ui";
import { client, useLoad } from "../../lib/api";
import { ENV_LABEL, describeError, useRun } from "../../lib/identity";
import { Meter } from "../Health";

/**
 * Tenancy & quotas (`#/identity/tenancy`, ADR 0012): each environment's restricted Incus project,
 * its restrictions and limit use as Incus reports them, and the quotas of organizations,
 * projects and environments. Everything shown, judged and allowed comes from the server.
 */

const GIB = 1024 ** 3;
const gib = (bytes: number) => {
  const v = bytes / GIB;
  return v >= 10 || Number.isInteger(v) ? `${Math.round(v)} GiB` : `${v.toFixed(1)} GiB`;
};

type Dim = "instances" | "cpu" | "memory" | "disk";
const DIMS: { dim: Dim; label: string; field: keyof QuotaLimits; unit: string }[] = [
  { dim: "instances", label: "Instances", field: "instances", unit: "" },
  { dim: "cpu", label: "CPUs", field: "cpu", unit: "" },
  { dim: "memory", label: "Memory", field: "memoryGib", unit: "GiB" },
  { dim: "disk", label: "Disk", field: "diskGib", unit: "GiB" },
];

const usedOf = (u: QuotaUsage, d: Dim) => (d === "instances" ? u.instances : d === "cpu" ? u.cpu : d === "memory" ? u.memoryBytes : u.diskBytes);
/** A limit in the same units as the use (bytes for memory and disk). */
const limitOf = (q: QuotaLimits | null | undefined, d: Dim): number | null => {
  const v = q?.[DIMS.find((x) => x.dim === d)!.field];
  if (v == null) return null;
  return d === "memory" || d === "disk" ? v * GIB : v;
};
const show = (d: Dim, v: number) => (d === "memory" || d === "disk" ? gib(v) : String(v));
const noun = (d: Dim, n: number) => (d === "instances" ? (n === 1 ? "instance" : "instances") : d === "cpu" ? (n === 1 ? "CPU" : "CPUs") : d === "memory" ? "memory" : "disk");

/** "3 of 10 instances", "6 GiB of 32 GiB memory". */
function usageText(d: Dim, used: number, limit: number) {
  return `${show(d, used)} of ${show(d, limit)} ${noun(d, limit)}`;
}

const isLimited = (q: QuotaLimits | null | undefined) => DIMS.some((x) => q?.[x.field] != null);

/** Quota use as meters: one per limited dimension. */
function QuotaUse({ quota, usage, hidden, testId }: { quota?: QuotaLimits | null; usage?: QuotaUsage | null; hidden?: string; testId: string }) {
  if (!isLimited(quota)) return <p className="text-sm text-base-content/70" data-testid={`${testId}-none`}>No quota.</p>;
  if (!usage) {
    const limits = DIMS.filter((x) => quota?.[x.field] != null).map((x) => `${show(x.dim, limitOf(quota, x.dim)!)} ${noun(x.dim, limitOf(quota, x.dim)!)}`);
    return (
      <div className="flex flex-col gap-1 text-sm" data-testid={`${testId}-limits`}>
        <p>Quota: {limits.join(", ")}.</p>
        {hidden && <p className="text-base-content/70">{hidden}</p>}
      </div>
    );
  }
  return (
    <div className="grid gap-3 sm:grid-cols-2" data-testid={testId}>
      {DIMS.filter((x) => quota?.[x.field] != null).map((x) => {
        const limit = limitOf(quota, x.dim)!;
        const used = usedOf(usage, x.dim);
        return <Meter key={x.dim} label={x.label} used={used} total={Math.max(limit, 1)} text={usageText(x.dim, used, limit)} testId={`${testId}-${x.dim}`} />;
      })}
    </div>
  );
}

/** What Incus reports for an environment's project: a meter where it has a limit, the use in words where not. */
function IncusUse({ use }: { use: TenancyProjectUse }) {
  const rows: { dim: Dim; label: string; r: TenancyResource }[] = [
    { dim: "instances", label: "Instances", r: use.instances },
    { dim: "cpu", label: "CPUs", r: use.cpu },
    { dim: "memory", label: "Memory", r: use.memory },
    { dim: "disk", label: "Disk", r: use.disk },
  ];
  return (
    <div className="grid gap-3 sm:grid-cols-2" data-testid="env-incus-use">
      {rows.map(({ dim, label, r }) => r.limit != null
        ? <Meter key={dim} label={label} used={r.used} total={Math.max(r.limit, 1)} text={usageText(dim, r.used, r.limit)} testId={`env-use-${dim}`} />
        : (
          <div key={dim} className="flex items-baseline justify-between gap-3 text-sm" data-testid={`env-use-${dim}`}>
            <span>{label}</span>
            <span className="tabular-nums text-base-content/70">{show(dim, r.used)} in use, no limit</span>
          </div>
        ))}
    </div>
  );
}

const STATE: Record<TenancyIncusState, { label: string; tone: Tone }> = {
  created: { label: "Created by Apelora", tone: "ok" },
  adopted: { label: "Adopted", tone: "ok" },
  pending: { label: "Not prepared", tone: "warn" },
};
const FINDING_TONE: Record<TenancyFinding["severity"], Tone> = { info: "info", warning: "warn", error: "error" };

// ---- the quota dialog -----------------------------------------------------------------------------

type Target = { kind: QuotaScopeKind; id: string; name: string };

function useFocusWhenShown(ref: { current: HTMLElement | null }) {
  useEffect(() => {
    let frames = 30;
    let id = 0;
    const focus = () => {
      const el = ref.current;
      el?.focus();
      if (el && document.activeElement !== el && frames-- > 0) id = requestAnimationFrame(focus);
    };
    focus();
    return () => cancelAnimationFrame(id);
  }, [ref]);
}

const SCOPE_NOUN: Record<QuotaScopeKind, string> = { organization: "organization", project: "project", environment: "environment" };

/** Sets one scope's quota. Empty fields mean no limit; the server checks the bound above. */
function QuotaForm({ target, onAuth, onClose, onSaved }: { target: Target; onAuth: (m: string) => void; onClose: () => void; onSaved: (r: QuotaChange) => void }) {
  const [view, setView] = useState<QuotaView | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [values, setValues] = useState<Record<string, string>>({});
  const first = useRef<HTMLInputElement>(null);
  const base = useId();
  const { busy, error, run } = useRun(onAuth);
  useFocusWhenShown(first);
  useEffect(() => {
    let live = true;
    client.request("quota", { params: { scope: target.kind, id: target.id } })
      .then((v) => {
        if (!live) return;
        setView(v);
        setValues(Object.fromEntries(DIMS.map((x) => [x.field, v.quota?.[x.field] != null ? String(v.quota[x.field]) : ""])));
      })
      .catch((e: unknown) => { if (live) setLoadError(e instanceof Error ? e.message : String(e)); });
    return () => { live = false; };
  }, [target.kind, target.id]);
  const parsed = DIMS.map((x) => {
    const raw = (values[x.field] ?? "").trim();
    const n = raw === "" ? null : Number(raw);
    return { ...x, raw, n, bad: raw !== "" && (!Number.isInteger(n) || (n ?? 0) < 0) };
  });
  const invalid = parsed.some((p) => p.bad);
  const submit = () => void run(async () => {
    const body: QuotaLimits = Object.fromEntries(parsed.map((p) => [p.field, p.n]));
    const r = await client.request("set_quota", { params: { scope: target.kind, id: target.id }, body });
    onSaved(r);
  });
  return (
    <form className="flex flex-col gap-3" onSubmit={(e) => { e.preventDefault(); if (!invalid) submit(); }}>
      <p className="text-sm text-base-content/70">Totals for the whole {SCOPE_NOUN[target.kind]}. Empty means no limit.</p>
      {loadError && <ErrorPanel message={loadError} testId="quota-load-error" />}
      <div className="grid gap-3 sm:grid-cols-2">
        {parsed.map((p, i) => {
          const bound = view ? limitOf(view.bound, p.dim) : null;
          const hint = p.bad ? "A whole number, 0 or more." : bound != null ? `Limit above: ${show(p.dim, bound)}` : "No limit above";
          return (
            <div key={p.field} className="flex flex-col gap-1 text-sm">
              <label htmlFor={`${base}-${p.field}`}>{p.label}{p.unit && ` (${p.unit})`}</label>
              <input ref={i === 0 ? first : undefined} id={`${base}-${p.field}`} className="input input-sm w-full tabular-nums" inputMode="numeric" autoComplete="off"
                value={values[p.field] ?? ""} onChange={(e) => setValues({ ...values, [p.field]: e.target.value })}
                aria-invalid={p.bad} aria-describedby={`${base}-${p.field}-hint`} placeholder="No limit" data-testid={`quota-${p.dim}`} />
              <span id={`${base}-${p.field}-hint`} className={`text-xs ${p.bad ? "text-error" : "text-base-content/60"}`}>{hint}</span>
            </div>
          );
        })}
      </div>
      {view?.usage && <p className="text-xs text-base-content/70" data-testid="quota-in-use">In use now: {view.usage.instances} {noun("instances", view.usage.instances)}, {view.usage.cpu} {noun("cpu", view.usage.cpu)}, {gib(view.usage.memoryBytes)} memory, {gib(view.usage.diskBytes)} disk.</p>}
      {view?.usageError && <p className="text-xs text-base-content/70">Use could not be read: {view.usageError}</p>}
      {error && <ErrorPanel message={error} testId="quota-error" />}
      <div className="modal-action mt-0">
        <button type="button" className="btn btn-sm btn-ghost" onClick={onClose}>Cancel</button>
        <button type="submit" className="btn btn-sm btn-primary" disabled={busy || invalid || !view} data-testid="quota-save">{busy ? "Saving…" : "Save quota"}</button>
      </div>
    </form>
  );
}

/** What the change did in Incus, in one sentence per kind of result. */
function savedMessage(target: Target, r: QuotaChange): { tone: Tone; text: string } {
  const failed = r.environments.filter((e) => e.result === "failed");
  const skipped = r.environments.filter((e) => e.result === "skipped");
  const applied = r.environments.filter((e) => e.result === "applied" || e.result === "unchanged");
  const parts = [r.quota ? `Quota of ${target.name} saved.` : `Quota of ${target.name} removed.`];
  if (applied.length) parts.push(`Incus limits set on ${applied.length} ${applied.length === 1 ? "environment" : "environments"}.`);
  if (skipped.length) parts.push(`Not set in Incus for ${skipped.map((e) => e.environment).join(", ")}: ${skipped[0]?.detail ?? ""}.`);
  if (failed.length) parts.push(`Incus refused ${failed.map((e) => `${e.environment} (${e.detail})`).join(", ")}. Apelora still enforces the quota.`);
  return { tone: failed.length ? "warn" : "ok", text: parts.join(" ") };
}

// ---- environments, projects, organizations -------------------------------------------------------

function Environment({ env, onQuota, onPrepare, busy }: { env: TenancyEnvironment; onQuota: () => void; onPrepare: () => void; busy: boolean }) {
  const state = STATE[env.incusState];
  const restrictions = Object.entries(env.incus?.restrictions ?? {}).sort(([a], [b]) => a.localeCompare(b));
  return (
    <li className="flex flex-col gap-3 py-3" data-testid="tenancy-environment">
      <div className="flex flex-wrap items-start justify-between gap-2">
        <div className="flex min-w-0 flex-col gap-1">
          <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
            <h4 className="font-medium">{env.name}</h4>
            <Badge tone="neutral">{ENV_LABEL[env.kind]}</Badge>
            <Badge tone={state.tone} testId="env-state">{state.label}</Badge>
          </div>
          <p className="text-sm text-base-content/70">Incus project <span className="font-mono text-xs text-base-content" data-testid="env-incus-project">{env.incusProject ?? "none"}</span></p>
        </div>
        <div className="flex flex-wrap justify-end gap-2">
          {env.incusState === "pending" && env.canPrepare && <button type="button" className="btn btn-xs btn-outline whitespace-nowrap" disabled={busy} onClick={onPrepare} data-testid="env-prepare">{busy ? "Preparing…" : "Prepare Incus project"}</button>}
          {env.canSetQuota && <button type="button" className="btn btn-xs btn-outline whitespace-nowrap" onClick={onQuota} aria-label={`Set quota of environment ${env.name}`} data-testid="env-quota">Set quota…</button>}
        </div>
      </div>
      {env.incusState === "pending" && env.incusDetail && <p className="text-sm text-base-content/70" data-testid="env-detail">{sentence(env.incusDetail)}.</p>}
      {isLimited(env.quota) && <QuotaUse quota={env.quota} testId="env-quota-own" />}
      {env.incus ? <IncusUse use={env.incus.usage} /> : env.incusError && <p className="text-sm text-base-content/70" data-testid="env-incus-error">Not read from Incus: {env.incusError}</p>}
      {env.findings.length > 0 && (
        <ul className="flex flex-col gap-1" data-testid="env-findings">
          {env.findings.map((f) => <li key={f.message} className="flex items-start gap-2 text-sm"><Badge tone={FINDING_TONE[f.severity]}>{f.severity === "info" ? "Note" : sentence(f.severity)}</Badge><span className="min-w-0 break-words">{f.message}</span></li>)}
        </ul>
      )}
      {restrictions.length > 0 && (
        <details className="text-sm" data-testid="env-restrictions">
          <summary className="cursor-pointer text-base-content/80">Restrictions and features ({restrictions.length})</summary>
          <dl className="mt-2 grid grid-cols-1 gap-x-4 gap-y-1 rounded-box border border-base-300 p-3 font-mono text-xs sm:grid-cols-2">
            {restrictions.map(([k, v]) => (
              <div key={k} className="flex min-w-0 items-baseline justify-between gap-3">
                <dt className="truncate" title={k}>{k}</dt>
                <dd className="shrink-0 text-base-content/70">{String(v)}</dd>
              </div>
            ))}
          </dl>
        </details>
      )}
    </li>
  );
}

function Project({ project, onQuota, envActions }: { project: TenancyProject; onQuota: (t: Target) => void; envActions: (e: TenancyEnvironment) => { onPrepare: () => void; busy: boolean } }) {
  return (
    <li className="flex flex-col gap-3 py-4" data-testid="tenancy-project">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h3 className="font-semibold">{project.name} <span className="font-mono text-xs font-normal text-base-content/60">{project.slug}</span></h3>
        {project.canSetQuota && <button type="button" className="btn btn-xs btn-outline whitespace-nowrap" onClick={() => onQuota({ kind: "project", id: project.id, name: project.name })} aria-label={`Set quota of project ${project.name}`} data-testid="project-quota">Set quota…</button>}
      </div>
      <QuotaUse quota={project.quota} usage={project.usage} hidden="Use unknown: an environment could not be read." testId="project-quota-use" />
      {project.environments.length === 0
        ? <p className="text-sm text-base-content/70">No environments yet.</p>
        : <ul className="divide-y divide-base-300 border-l-2 border-base-300 pl-4">{project.environments.map((e) => <Environment key={e.id} env={e} onQuota={() => onQuota({ kind: "environment", id: e.id, name: e.name })} {...envActions(e)} />)}</ul>}
    </li>
  );
}

function Organization({ org, onQuota, envActions }: { org: TenancyOrganization; onQuota: (t: Target) => void; envActions: (e: TenancyEnvironment) => { onPrepare: () => void; busy: boolean } }) {
  return (
    <li className="flex flex-col gap-3 rounded-box border border-base-300 px-4 py-4" data-testid="tenancy-organization">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h2 className="font-semibold">{org.name} <span className="font-mono text-xs font-normal text-base-content/60">{org.slug}</span></h2>
        {org.canSetQuota && <button type="button" className="btn btn-xs btn-outline whitespace-nowrap" onClick={() => onQuota({ kind: "organization", id: org.id, name: org.name })} aria-label={`Set quota of organization ${org.name}`} data-testid="org-quota">Set quota…</button>}
      </div>
      <QuotaUse quota={org.quota} usage={org.usage} hidden="Use hidden: you see only part of this organization." testId="org-quota-use" />
      {org.projects.length === 0
        ? <p className="text-sm text-base-content/70">No projects you can see.</p>
        : <ul className="divide-y divide-base-300">{org.projects.map((p) => <Project key={p.id} project={p} onQuota={onQuota} envActions={envActions} />)}</ul>}
    </li>
  );
}

export function TenancyView({ onAuth }: { onAuth: (m: string) => void }) {
  const [data, reload] = useLoad<TenancyOverview>(() => client.get("tenancy"), onAuth);
  const [target, setTarget] = useState<Target | null>(null);
  const [notice, setNotice] = useState<{ tone: Tone; text: string } | null>(null);
  const [preparing, setPreparing] = useState<string | null>(null);
  const prepare = async (env: TenancyEnvironment) => {
    setPreparing(env.id);
    setNotice(null);
    try {
      const r = await client.request("prepare_incus_project", { params: { id: env.id } });
      setNotice(r.state === "pending"
        ? { tone: "warn", text: `The Incus project of ${env.name} is still not prepared: ${r.detail}.` }
        : { tone: "ok", text: `${sentence(r.detail)}.` });
    } catch (e) {
      if (e instanceof ApiRequestError && e.unauthenticated) onAuth(e.message);
      setNotice({ tone: "error", text: describeError(e) });
    } finally {
      setPreparing(null);
      reload();
    }
  };
  const envActions = (e: TenancyEnvironment) => ({ onPrepare: () => void prepare(e), busy: preparing === e.id });
  return (
    <>
      <Section title="Tenancy & quotas" level={1} state={data} testId="tenancy" onRetry={reload}>
        {(o) => (
          <div className="flex flex-col gap-6">
            <div className="flex flex-col gap-2">
              <SectionHeader title="Tenancy & quotas" level={1} source={o.source} />
              <p className="text-sm text-base-content/70">Each environment runs in its own restricted Incus project. Deployments over a quota are refused.</p>
            </div>
            {!o.adminConfigured && <Notice tone="warn" testId="tenancy-no-admin">No Incus administrator identity: Apelora cannot create projects or set limits. Quotas are still checked.</Notice>}
            {notice && <Notice tone={notice.tone} testId="tenancy-notice">{notice.text}</Notice>}
            {o.organizations.length === 0
              ? <p className="text-sm text-base-content/70" data-testid="tenancy-empty">No organizations you can see.</p>
              : <ul className="flex flex-col gap-4">{o.organizations.map((org) => <Organization key={org.id} org={org} onQuota={(t) => { setNotice(null); setTarget(t); }} envActions={envActions} />)}</ul>}
            <Baseline baseline={o.baseline} />
          </div>
        )}
      </Section>
      <FormDialog open={!!target} title={target ? `Quota of ${target.name}` : "Quota"} onClose={() => setTarget(null)} testId="quota-dialog">
        {target && <QuotaForm key={`${target.kind}:${target.id}`} target={target} onAuth={onAuth} onClose={() => setTarget(null)} onSaved={(r) => { setNotice(savedMessage(target, r)); setTarget(null); reload(); }} />}
      </FormDialog>
    </>
  );
}

function Baseline({ baseline }: { baseline: Record<string, unknown> }): ReactNode {
  const rows = Object.entries(baseline).sort(([a], [b]) => a.localeCompare(b));
  return (
    <details className="text-sm" data-testid="tenancy-baseline">
      <summary className="cursor-pointer text-base-content/80">What a new environment's Incus project allows ({rows.length} {rows.length === 1 ? "setting" : "settings"})</summary>
      <dl className="mt-2 grid grid-cols-1 gap-x-4 gap-y-1 rounded-box border border-base-300 p-3 font-mono text-xs sm:grid-cols-2">
        {rows.map(([k, v]) => (
          <div key={k} className="flex min-w-0 items-baseline justify-between gap-3">
            <dt className="truncate" title={k}>{k}</dt>
            <dd className="shrink-0 text-base-content/70">{String(v)}</dd>
          </div>
        ))}
      </dl>
    </details>
  );
}
