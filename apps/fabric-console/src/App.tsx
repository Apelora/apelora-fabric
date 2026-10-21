import { useCallback, useEffect, useRef, useState } from "react";

import { ApiRequestError, completeOidcSignIn, type ClusterResponse, type Health, type Permission, type WhoAmI } from "@apelora/api-client";

import { Section, SectionHeader, StatusRow, TableFrame } from "./components/ui";
import { client, messageOf, readToken, useLoad, writeToken, type Load } from "./lib/api";
import { AdministrationView } from "./views/Administration";
import { AlertsAtAGlance, AlertsView } from "./views/Alerts";
import { ApplicationsView } from "./views/Applications";
import { AuditView } from "./views/Audit";
import { HealthView } from "./views/Health";
import { InstancesView } from "./views/Instances";
import { NetworksView } from "./views/Networks";
import { OperationsView } from "./views/Operations";
import { ServersView, isServerTab, type ServerTab } from "./views/servers/Servers";
import { UserMenu } from "./views/Account";
import { SetPasswordPage, setupTokenFromHash } from "./views/SetPassword";
import { SignInPage, type SignInReason } from "./views/SignIn";
import { StorageView } from "./views/Storage";
import { IdentityPage } from "./views/identity";
import { IDENTITY_PAGES, isIdentityPage, type IdentityPageId } from "./views/identity/pages";
import { holds } from "./lib/identity";
import { detailView, instanceRoute, jobRoute } from "./lib/routes";
import { InstanceDetailView } from "./views/InstanceDetail";
import { JobDetailView } from "./views/JobDetail";
import { AccountPage } from "./views/credentials";
import { ACCOUNT_PAGES, isAccountPage, type AccountPageId } from "./views/credentials/pages";

type ApiState = { kind: "checking" } | { kind: "up"; health: Health } | { kind: "down" };

/** Views, in navigation order; `group` puts a view under a heading. A view whose permission the caller lacks is not offered. */
const VIEWS = [
  { id: "overview", label: "Overview" },
  { id: "health", label: "Health", needs: "read-inventory" as Permission },
  { id: "servers", label: "Servers", needs: "read-inventory" as Permission },
  { id: "applications", label: "Applications", group: "Workloads" },
  { id: "instances", label: "Instances", group: "Workloads" },
  { id: "operations", label: "Operations", group: "Workloads" },
  // Networks and Storage check each section themselves: an application's viewers see its own.
  { id: "networks", label: "Networks" },
  { id: "storage", label: "Storage" },
  { id: "alerts", label: "Alerts", needs: "read-inventory" as Permission },
  // Identity & Access pages come here (their own group, see navSections).
  { id: "audit", label: "Audit", needs: "read-audit" as Permission },
  { id: "administration", label: "Administration" },
] as const;
type CoreViewId = (typeof VIEWS)[number]["id"];
type ViewId = CoreViewId | IdentityPageId | AccountPageId;
/** Views with one section use that section's header as their page title; Identity & Access pages carry their own. */
const SINGLE_SECTION = new Set<ViewId>(["applications", "instances", "operations", "alerts", "audit", ...IDENTITY_PAGES.map((p) => p.id), ...ACCOUNT_PAGES.map((p) => p.id)]);
const labelOf = (id: ViewId) => VIEWS.find((v) => v.id === id)?.label ?? IDENTITY_PAGES.find((p) => p.id === id)?.label ?? ACCOUNT_PAGES.find((p) => p.id === id)?.label;

/** Addresses from before the navigation was regrouped, and where they live now. */
const MOVED: Record<string, string> = {
  membership: "servers/machines",
  preflight: "servers/preflight",
  infrastructure: "networks",
};

/** The view, the Servers tab, and the whole address (detail pages under a view read it). */
type Route = { view: ViewId; tab: ServerTab; hash: string };

/** The view (and Servers tab) the address names; an old address is rewritten in place first. */
const routeFromHash = (): Route => {
  let path = window.location.hash.replace(/^#\/?/, "");
  const moved = MOVED[path.split("/")[0] ?? ""];
  if (moved) {
    path = moved;
    window.history.replaceState(null, "", `${window.location.pathname}${window.location.search}#/${moved}`);
  }
  const hash = window.location.hash;
  // Detail pages under a view (`#/instances/{project}/{name}`, `#/operations/{id}`).
  const detail = detailView(hash);
  if (detail) return { view: detail, tab: "members", hash };
  // A view may keep its filters after `?` (Audit).
  path = path.replace(/\?.*$/, "");
  // Identity & Access pages carry their own slash (`identity/users`).
  if (isIdentityPage(path) || isAccountPage(path)) return { view: path, tab: "members", hash };
  const [id, sub] = path.split("/");
  const tab: ServerTab = isServerTab(sub) ? sub : "members";
  return { view: (VIEWS.find((v) => v.id === id)?.id ?? "overview") as ViewId, tab, hash };
};

type NavView = { id: ViewId; label: string; group?: string };
const IDENTITY_GROUP = "Identity & Access";
/** Consecutive views of one group share a heading; Identity & Access comes before Audit and Administration. */
function navSections(views: readonly NavView[], identity: readonly NavView[]) {
  const out: { group?: string; items: NavView[] }[] = [];
  let placed = identity.length === 0;
  for (const v of views) {
    if (!placed && (v.id === "audit" || v.id === "administration")) {
      out.push({ group: IDENTITY_GROUP, items: [...identity] });
      placed = true;
    }
    const last = out[out.length - 1];
    if (last && last.group === v.group) last.items.push(v);
    else out.push({ group: v.group, items: [v] });
  }
  if (!placed) out.push({ group: IDENTITY_GROUP, items: [...identity] });
  return out;
}

/** The cluster members. Also reports what it loaded: the Incus version, or why Incus cannot be reached (signed-in detail the public health check does not give). */
function Members({ onAuth, onLoad }: { onAuth: (m: string) => void; onLoad: (state: Load<ClusterResponse>) => void }) {
  const [cluster, reload] = useLoad<ClusterResponse>(() => client.get("cluster"), onAuth);
  useEffect(() => onLoad(cluster), [cluster, onLoad]);
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

// A provider's code can be exchanged once; effects may run twice (StrictMode), so the page load
// shares one exchange.
let oidcReturn: ReturnType<typeof completeOidcSignIn> | null = null;
const finishOidcSignIn = () => (oidcReturn ??= completeOidcSignIn(new URL(window.location.href)));

export function App() {
  const [api, setApi] = useState<ApiState>({ kind: "checking" });
  const [authNeeded, setAuthNeeded] = useState<SignInReason | null>(null);
  // A token was just typed in: a 401 now means it was not accepted.
  const submitted = useRef(false);
  // The identity provider sent the browser back with an error: show it on the sign-in page.
  const providerError = useRef<string | null>(null);
  // The token in hand came from the identity provider just now: a refusal is not "session ended".
  const fromProvider = useRef(false);
  const [me, setMe] = useState<WhoAmI | null>(null);
  const [route, setRoute] = useState<Route>(routeFromHash);
  const view = route.view;
  // `#/set-password?token=…`: a person choosing their password from a setup link.
  const [setupToken, setSetupToken] = useState(() => setupTokenFromHash(window.location.hash));
  const [attempt, setAttempt] = useState(0);
  // What the cluster request said (signed in): the running Incus version or the reason it is unreachable.
  const [clusterLoad, setClusterLoad] = useState<Load<ClusterResponse> | null>(null);

  useEffect(() => {
    const onHash = () => { setRoute(routeFromHash()); setSetupToken(setupTokenFromHash(window.location.hash)); };
    window.addEventListener("hashchange", onHash);
    return () => window.removeEventListener("hashchange", onHash);
  }, []);

  const onAuth = useCallback((msg: string) => {
    const had = readToken() !== null;
    writeToken(null);
    setMe(null);
    setAuthNeeded(
      providerError.current ? { error: providerError.current }
        : fromProvider.current ? { error: `You signed in, but Apelora did not accept the identity provider's token: ${msg}` }
        : submitted.current ? "rejected" : had ? "ended" : "required",
    );
    submitted.current = false;
    fromProvider.current = false;
    providerError.current = null;
  }, []);

  const check = useCallback(() => {
    setApi({ kind: "checking" });
    setAuthNeeded(null);
    setClusterLoad(null);
    client.get("health")
      .then((health) => {
        setApi({ kind: "up", health });
        setAttempt((n) => n + 1);
        client.get("whoami")
          .then((who) => { submitted.current = false; fromProvider.current = false; setMe(who); })
          .catch((e: unknown) => { if (e instanceof ApiRequestError && e.unauthenticated) onAuth(e.message); });
      })
      .catch(() => setApi({ kind: "down" }));
  }, [onAuth]);

  // Back from the identity provider (?code&state): finish signing in, then tidy the address.
  const [returning, setReturning] = useState(() => new URL(window.location.href).searchParams.has("state"));
  useEffect(() => {
    if (!returning) { check(); return; }
    finishOidcSignIn()
      .then((r) => {
        window.history.replaceState(null, "", `${window.location.pathname}${r?.returnTo ?? ""}`);
        setRoute(routeFromHash()); // replaceState fires no hashchange
        if (r) { writeToken(r.token); fromProvider.current = true; }
        setReturning(false);
      })
      .catch((e: unknown) => {
        window.history.replaceState(null, "", window.location.pathname);
        providerError.current = messageOf(e);
        setReturning(false);
      });
  }, [returning, check]);

  // Ends the session on the server first (a password session is revoked there), then forgets
  // the token in this tab whatever the answer.
  const signOut = useCallback(async () => {
    let failed: string | null = null;
    try { await client.request("sign_out", {}); } catch (e) {
      if (!(e instanceof ApiRequestError && e.unauthenticated)) failed = messageOf(e);
    }
    writeToken(null);
    setMe(null);
    setAuthNeeded(failed ? { error: `You are signed out in this tab, but the control plane could not be told (${failed}). Your session ends on its own within an hour.` } : "signed-out");
  }, []);

  const up = api.kind === "up" ? api.health : null;
  const controlPlane =
    api.kind === "checking"
      ? { tone: "pending" as const, value: "Checking…" }
      : api.kind === "down"
        ? { tone: "off" as const, value: "Not reachable" }
        : { tone: "ok" as const, value: `Running · ${api.health.provider === "mock" ? "mock data" : "Incus"}` };
  const incusConnected = !!up?.incus.connected;
  const incusVersion = clusterLoad?.kind === "ok" ? clusterLoad.data.server.serverVersion : null;
  const incusError = clusterLoad?.kind === "error" ? clusterLoad.message : null;
  const incus = up
    ? incusConnected
      ? { tone: "ok" as const, value: `Connected${incusVersion ? ` · Incus ${incusVersion}` : ""}` }
      : { tone: "off" as const, value: up.provider === "incus" ? "Unreachable" : "Not connected" }
    : { tone: "pending" as const, value: "Unknown" };
  const badge = incusConnected ? "Connected" : up?.provider === "mock" ? "Mock data" : "Not connected";
  const ready = !!up && !authNeeded;
  const views = VIEWS.filter((v) => !("needs" in v) || !me || me.permissions.includes(v.needs));
  // Identity & Access pages are offered only once we know what the caller holds.
  const identityViews = me ? IDENTITY_PAGES.filter((p) => holds(me, ...p.any)) : [];
  const current: ViewId = views.some((v) => v.id === view) || identityViews.some((p) => p.id === view) || (!!me && isAccountPage(view)) ? view : "overview";

  if (setupToken !== null) {
    return (
      <div className="h-full overflow-auto bg-base-100 text-base-content">
        <SetPasswordPage token={setupToken} health={up} onDone={() => { window.location.hash = "#/"; }} />
      </div>
    );
  }

  if (authNeeded && api.kind !== "down") {
    return (
      <div className="h-full overflow-auto bg-base-100 text-base-content">
        <SignInPage reason={authNeeded} health={up} onToken={(t) => { writeToken(t); submitted.current = true; check(); }} />
      </div>
    );
  }

  return (
    <div className="flex h-full flex-col overflow-hidden bg-base-100 text-base-content">
      <header className="flex min-h-12 shrink-0 flex-wrap items-center justify-between gap-x-3 gap-y-1 border-b border-base-300 px-4">
        <div className="flex min-w-0 items-center gap-3">
          <img src="/logo.svg" alt="" className="size-6" />
          <span className="whitespace-nowrap text-sm font-semibold">Apelora Fabric Console</span>
          <span className="badge badge-sm badge-ghost" data-testid="mock-badge" title="Shows whether the data below comes from a real Incus cluster.">{badge}</span>
        </div>
        {me && <UserMenu me={me} onAuth={onAuth} onSignOut={() => void signOut()} />}
      </header>

      <div className="flex min-h-0 flex-1 flex-col md:flex-row">
        {ready && (
          // Phones: one wrapping row of links (group names for screen readers only); from `md` a sidebar
          // with the groups' links indented under their headings.
          <nav aria-label="Console" className="flex shrink-0 flex-wrap items-center border-b border-base-300 px-2 py-1 md:block md:w-56 md:overflow-y-auto md:border-r md:border-b-0 md:px-0 md:py-2">
            {navSections(views, identityViews).map((section, i) => {
              const list = (
                <ul className={`menu menu-horizontal menu-sm flex-wrap p-0 md:w-full md:menu-vertical md:flex-nowrap md:gap-0.5 md:px-2 max-md:contents ${section.group ? "md:pl-4" : ""}`}>
                  {section.items.map((v) => (
                    <li key={v.id}>
                      <a href={`#/${v.id}`} aria-current={current === v.id ? "page" : undefined} className={current === v.id ? "menu-active" : undefined}>{v.label}</a>
                    </li>
                  ))}
                </ul>
              );
              if (!section.group) return <div key={`top-${i}`} className="contents md:block">{list}</div>;
              const id = `nav-${section.group.toLowerCase().replace(/[^a-z]+/g, "-")}`;
              return (
                <div key={id} role="group" aria-labelledby={id} data-testid={section.group === IDENTITY_GROUP ? "nav-identity" : id} className="max-md:contents md:block md:pt-2">
                  <p id={id} className="pr-1 pl-3 text-xs font-semibold uppercase tracking-wide text-base-content/70 max-md:sr-only md:px-4 md:pt-1 md:pb-1">{section.group}</p>
                  {list}
                </div>
              );
            })}
          </nav>
        )}

        <main className="min-w-0 flex-1 overflow-auto">
          <div className="mx-auto flex w-full max-w-5xl flex-col gap-6 px-4 py-8">
            {(!ready || !SINGLE_SECTION.has(current)) && <h1 className="text-lg font-semibold">{labelOf(current)}</h1>}

            {current === "overview" && (
              <section aria-label="Service status" aria-live="polite" className="divide-y divide-base-300 rounded-box border border-base-300" data-testid="status">
                <StatusRow label="Control plane" {...controlPlane} />
                <StatusRow label="Incus cluster" {...incus} />
                {up && <StatusRow label="Authentication" tone={up.auth === "tokens" ? "ok" : "off"} value={up.auth === "tokens" ? "Sign-in required" : "Disabled (development only)"} />}
              </section>
            )}


            {current === "overview" && !incusConnected && (
              <section className="flex flex-col items-start gap-3 rounded-box border border-dashed border-base-300 p-6" data-testid="empty-state">
                <h2 className="font-semibold">No cluster connected yet</h2>
                {api.kind === "down" ? (
                  <p className="text-sm text-base-content/70">
                    Start the control plane with <code className="rounded bg-base-200 px-1 py-0.5 text-xs">cargo run -p apelora-control-plane</code>, then check again.
                  </p>
                ) : up?.provider === "incus" ? (
                  <p className="text-sm text-base-content/70">The control plane is configured for Incus but cannot reach it{incusError ? `: ${incusError}` : "."}</p>
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
                {current === "overview" && me?.permissions.includes("read-inventory") && <AlertsAtAGlance onAuth={onAuth} />}
                {current === "overview" && <Members onAuth={onAuth} onLoad={setClusterLoad} />}
                {current === "health" && <HealthView onAuth={onAuth} />}
                {current === "applications" && <ApplicationsView me={me} onAuth={onAuth} />}
                {current === "instances" && (() => {
                  const r = instanceRoute(route.hash);
                  return r ? <InstanceDetailView key={route.hash} project={r.project} name={r.name} me={me} onAuth={onAuth} /> : <InstancesView me={me} onAuth={onAuth} />;
                })()}
                {current === "servers" && <ServersView tab={route.tab} me={me} onAuth={onAuth} />}
                {current === "networks" && <NetworksView me={me} onAuth={onAuth} />}
                {current === "storage" && <StorageView me={me} onAuth={onAuth} />}
                {current === "audit" && <AuditView onAuth={onAuth} />}
                {current === "alerts" && <AlertsView onAuth={onAuth} />}
                {current === "operations" && (() => {
                  const id = jobRoute(route.hash);
                  return id ? <JobDetailView key={id} id={id} onAuth={onAuth} /> : <OperationsView onAuth={onAuth} />;
                })()}
                {current === "administration" && <AdministrationView me={me} onAuth={onAuth} />}
                {me && isIdentityPage(current) && <IdentityPage id={current} me={me} onAuth={onAuth} />}
                {me && isAccountPage(current) && <AccountPage id={current} me={me} onAuth={onAuth} />}
              </div>
            )}
          </div>
        </main>
      </div>
    </div>
  );
}
