import { useCallback, useEffect, useState } from "react";

import { ApiRequestError, type ClusterResponse, type Health, type Permission, type WhoAmI } from "@apelora/api-client";

import { Section, SectionHeader, StatusRow, TableFrame } from "./components/ui";
import { client, useLoad, writeToken } from "./lib/api";
import { AdministrationView } from "./views/Administration";
import { ApplicationsView } from "./views/Applications";
import { AuditView } from "./views/Audit";
import { InfrastructureView } from "./views/Infrastructure";
import { InstancesView } from "./views/Instances";
import { MembershipView } from "./views/Membership";
import { OperationsView } from "./views/Operations";
import { PreflightView } from "./views/Preflight";
import { IdentityPage } from "./views/identity";
import { IDENTITY_PAGES, isIdentityPage, type IdentityPageId } from "./views/identity/pages";
import { holds } from "./lib/identity";

type ApiState = { kind: "checking" } | { kind: "up"; health: Health } | { kind: "down" };

/** Views, in navigation order. A view whose permission the caller lacks is not offered. */
const VIEWS = [
  { id: "overview", label: "Overview" },
  { id: "applications", label: "Applications" },
  { id: "instances", label: "Instances" },
  { id: "operations", label: "Operations" },
  { id: "preflight", label: "Preflight" },
  { id: "infrastructure", label: "Networks & storage" },
  { id: "membership", label: "Membership", needs: "manage-infrastructure" as Permission },
  { id: "audit", label: "Audit log", needs: "read-audit" as Permission },
  { id: "administration", label: "Administration" },
] as const;
type CoreViewId = (typeof VIEWS)[number]["id"];
type ViewId = CoreViewId | IdentityPageId;
/** Views with one section use that section's header as their page title; Identity & Access pages carry their own. */
const SINGLE_SECTION = new Set<ViewId>(["applications", "instances", "operations", "preflight", "audit", ...IDENTITY_PAGES.map((p) => p.id)]);
const labelOf = (id: ViewId) => VIEWS.find((v) => v.id === id)?.label ?? IDENTITY_PAGES.find((p) => p.id === id)?.label;

const viewFromHash = (): ViewId => {
  const id = window.location.hash.replace(/^#\/?/, "");
  if (isIdentityPage(id)) return id;
  return (VIEWS.find((v) => v.id === id)?.id ?? "overview") as ViewId;
};

function SignIn({ message, onSubmit }: { message: string; onSubmit: (token: string) => void }) {
  const [value, setValue] = useState("");
  return (
    <form className="flex flex-col gap-3 rounded-box border border-base-300 p-6" data-testid="sign-in" onSubmit={(e) => { e.preventDefault(); if (value.trim()) onSubmit(value.trim()); }}>
      <h2 className="font-semibold">Sign in with an API token</h2>
      <p className="text-sm text-base-content/70">{message} An administrator creates tokens with <code className="rounded bg-base-200 px-1 py-0.5 text-xs">apelora-control-plane token create</code>. It is kept only for this browser tab.</p>
      <label className="flex flex-col gap-1 text-sm">
        <span>API token</span>
        <input type="password" autoComplete="off" spellCheck={false} className="input input-sm w-full max-w-md font-mono" value={value} onChange={(e) => setValue(e.target.value)} data-testid="token-input" />
      </label>
      <button type="submit" className="btn btn-sm btn-primary self-start" disabled={!value.trim()}>Sign in</button>
    </form>
  );
}

function Members({ onAuth }: { onAuth: (m: string) => void }) {
  const [cluster, reload] = useLoad<ClusterResponse>(() => client.get("cluster"), onAuth);
  return (
    <Section title="Cluster members" state={cluster} testId="members" onRetry={reload}>
      {(c) => (
        <>
          <SectionHeader title="Cluster members" count={c.members.length} source={c.source} />
          {c.members.length === 0 ? (
            <p className="text-sm text-base-content/70">{c.server.clustered ? "The cluster reports no members." : "This Incus server is not clustered."}</p>
          ) : (
            <TableFrame>
              <thead><tr><th>Member</th><th>Status</th><th>Roles</th><th>Failure domain</th><th>Architecture</th></tr></thead>
              <tbody>
                {c.members.map((m) => (
                  <tr key={m.name} data-testid="member-row">
                    <td className="font-medium">{m.name}</td>
                    <td title={m.message}><span className={`status ${m.status === "Online" ? "status-success" : "status-error"} mr-2`} aria-hidden="true" />{m.status}</td>
                    <td>{m.roles.join(", ") || "—"}</td>
                    <td>{m.failureDomain || "—"}</td>
                    <td>{m.architecture}</td>
                  </tr>
                ))}
              </tbody>
            </TableFrame>
          )}
        </>
      )}
    </Section>
  );
}

export function App() {
  const [api, setApi] = useState<ApiState>({ kind: "checking" });
  const [authNeeded, setAuthNeeded] = useState<string | null>(null);
  const [me, setMe] = useState<WhoAmI | null>(null);
  const [view, setView] = useState<ViewId>(viewFromHash);
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    const onHash = () => setView(viewFromHash());
    window.addEventListener("hashchange", onHash);
    return () => window.removeEventListener("hashchange", onHash);
  }, []);

  const onAuth = useCallback((msg: string) => { writeToken(null); setMe(null); setAuthNeeded(msg); }, []);

  const check = useCallback(() => {
    setApi({ kind: "checking" });
    setAuthNeeded(null);
    client.get("health")
      .then((health) => {
        setApi({ kind: "up", health });
        setAttempt((n) => n + 1);
        client.get("whoami")
          .then(setMe)
          .catch((e: unknown) => { if (e instanceof ApiRequestError && e.unauthenticated) onAuth(e.message); });
      })
      .catch(() => setApi({ kind: "down" }));
  }, [onAuth]);

  useEffect(check, [check]);

  const up = api.kind === "up" ? api.health : null;
  const controlPlane =
    api.kind === "checking"
      ? { tone: "pending" as const, value: "Checking…" }
      : api.kind === "down"
        ? { tone: "off" as const, value: "Not reachable" }
        : { tone: "ok" as const, value: `Running · v${api.health.version} · ${api.health.provider === "mock" ? "mock data" : "Incus"}` };
  const incusConnected = !!up?.incus.connected;
  const incus = up
    ? incusConnected
      ? { tone: "ok" as const, value: `Connected${up.incus.serverVersion ? ` · Incus ${up.incus.serverVersion}` : ""}` }
      : { tone: "off" as const, value: up.provider === "incus" && up.incus.error ? "Unreachable" : "Not connected" }
    : { tone: "pending" as const, value: "Unknown" };
  const badge = incusConnected ? "Connected" : up?.provider === "mock" ? "Mock data" : "Not connected";
  const ready = !!up && !authNeeded;
  const views = VIEWS.filter((v) => !("needs" in v) || !me || me.permissions.includes(v.needs));
  // Identity & Access pages are offered only once we know what the caller holds.
  const identityViews = me ? IDENTITY_PAGES.filter((p) => holds(me, ...p.any)) : [];
  const current: ViewId = views.some((v) => v.id === view) || identityViews.some((p) => p.id === view) ? view : "overview";

  return (
    <div className="flex h-full flex-col overflow-hidden bg-base-100 text-base-content">
      <header className="flex h-12 shrink-0 items-center justify-between gap-3 border-b border-base-300 px-4">
        <div className="flex min-w-0 items-center gap-3">
          <img src="/logo.svg" alt="" className="size-6" />
          <span className="whitespace-nowrap text-sm font-semibold">Apelora Fabric Console</span>
          <span className="badge badge-sm badge-ghost" data-testid="mock-badge" title="Shows whether the data below comes from a real Incus cluster.">{badge}</span>
        </div>
        {me && <span className="hidden truncate text-xs text-base-content/60 sm:inline" data-testid="whoami">{me.name} · {me.role}</span>}
      </header>

      <div className="flex min-h-0 flex-1 flex-col md:flex-row">
        {ready && (
          <nav aria-label="Console" className="shrink-0 border-b border-base-300 md:w-56 md:overflow-y-auto md:border-r md:border-b-0">
            <ul className="menu menu-horizontal menu-sm w-full flex-wrap md:menu-vertical md:flex-nowrap md:gap-0.5 md:p-2">
              {views.map((v) => (
                <li key={v.id}>
                  <a href={`#/${v.id}`} aria-current={current === v.id ? "page" : undefined} className={current === v.id ? "menu-active" : undefined}>{v.label}</a>
                </li>
              ))}
            </ul>
            {identityViews.length > 0 && (
              <div role="group" aria-labelledby="nav-identity" data-testid="nav-identity">
                <p id="nav-identity" className="px-4 pt-2 text-xs font-semibold uppercase tracking-wide text-base-content/70 md:px-6 md:pt-3">Identity &amp; Access</p>
                <ul className="menu menu-horizontal menu-sm w-full flex-wrap md:menu-vertical md:flex-nowrap md:gap-0.5 md:p-2">
                  {identityViews.map((p) => (
                    <li key={p.id}>
                      <a href={`#/${p.id}`} aria-current={current === p.id ? "page" : undefined} className={current === p.id ? "menu-active" : undefined}>{p.label}</a>
                    </li>
                  ))}
                </ul>
              </div>
            )}
          </nav>
        )}

        <main className="min-w-0 flex-1 overflow-auto">
          <div className="mx-auto flex w-full max-w-5xl flex-col gap-6 px-4 py-8">
            {(!ready || !SINGLE_SECTION.has(current)) && <h1 className="text-lg font-semibold">{labelOf(current)}</h1>}

            {current === "overview" && (
              <section aria-label="Service status" aria-live="polite" className="divide-y divide-base-300 rounded-box border border-base-300" data-testid="status">
                <StatusRow label="Control plane" {...controlPlane} />
                <StatusRow label="Incus cluster" {...incus} />
                {up && <StatusRow label="Authentication" tone={up.auth === "tokens" ? "ok" : "off"} value={up.auth === "tokens" ? "API tokens required" : "Disabled (development only)"} />}
              </section>
            )}

            {authNeeded && <SignIn message={authNeeded} onSubmit={(t) => { writeToken(t); check(); }} />}

            {current === "overview" && !incusConnected && (
              <section className="flex flex-col items-start gap-3 rounded-box border border-dashed border-base-300 p-6" data-testid="empty-state">
                <h2 className="font-semibold">No cluster connected yet</h2>
                {api.kind === "down" ? (
                  <p className="text-sm text-base-content/70">
                    Start the control plane with <code className="rounded bg-base-200 px-1 py-0.5 text-xs">cargo run -p apelora-control-plane</code>, then check again.
                  </p>
                ) : up?.provider === "incus" ? (
                  <p className="text-sm text-base-content/70">The control plane is configured for Incus but cannot reach it: {up.incus.error}</p>
                ) : (
                  <p className="text-sm text-base-content/70">
                    The tables show mock data. Set <code className="rounded bg-base-200 px-1 py-0.5 text-xs">APELORA_INCUS_URL</code> and the certificate paths for the control plane to read a real cluster.
                  </p>
                )}
                <button type="button" className={`btn btn-sm ${authNeeded ? "btn-outline" : "btn-primary"}`} onClick={check} disabled={api.kind === "checking"}>
                  Check again
                </button>
              </section>
            )}

            {ready && (
              <div key={`${current}-${attempt}`} className="flex flex-col gap-6">
                {current === "overview" && <Members onAuth={onAuth} />}
                {current === "applications" && <ApplicationsView onAuth={onAuth} />}
                {current === "instances" && <InstancesView me={me} onAuth={onAuth} />}
                {current === "preflight" && <PreflightView onAuth={onAuth} />}
                {current === "infrastructure" && <InfrastructureView onAuth={onAuth} />}
                {current === "membership" && <MembershipView onAuth={onAuth} />}
                {current === "audit" && <AuditView onAuth={onAuth} />}
                {current === "operations" && <OperationsView onAuth={onAuth} />}
                {current === "administration" && <AdministrationView me={me} health={up} onAuth={onAuth} />}
                {me && isIdentityPage(current) && <IdentityPage id={current} me={me} onAuth={onAuth} />}
              </div>
            )}
          </div>
        </main>
      </div>
    </div>
  );
}
