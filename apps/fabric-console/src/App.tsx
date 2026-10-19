import { useCallback, useEffect, useState } from "react";

/** Shapes returned by the Apelora control plane (`/api/v1`). */
type Provider = "mock" | "incus";
type Source = { provider: Provider; observedAt: string };
type Health = {
  status: string;
  version: string;
  provider: Provider;
  auth?: "tokens" | "disabled-development";
  incus: { connected: boolean; apiVersion?: string; serverVersion?: string | null; clustered?: boolean; error?: string };
};
type Member = { name: string; url: string; roles: string[]; status: string; message: string; architecture: string; failureDomain: string };
type Instance = { name: string; project: string; kind: "container" | "virtual-machine" | "unknown"; status: string; location: string; architecture: string };
type ClusterRes = { source: Source; server: { clustered: boolean; serverVersion: string | null }; members: Member[] };
type ListRes<T> = { source: Source; items: T[] };
type ApiError = { error: { code: string; message: string } };

type Load<T> = { kind: "loading" } | { kind: "ok"; data: T } | { kind: "error"; message: string };
type ApiState = { kind: "checking" } | { kind: "up"; health: Health } | { kind: "down" };

/** The API token lives only in this browser tab (sessionStorage), never in localStorage. */
const TOKEN_KEY = "apelora.apiToken";
const readToken = (): string | null => { try { return sessionStorage.getItem(TOKEN_KEY); } catch { return null; } };
const writeToken = (t: string | null) => { try { if (t) sessionStorage.setItem(TOKEN_KEY, t); else sessionStorage.removeItem(TOKEN_KEY); } catch { /* storage unavailable */ } };

class Unauthenticated extends Error {}

async function getJson<T>(path: string): Promise<T> {
  const token = readToken();
  const res = await fetch(path, { headers: { accept: "application/json", ...(token ? { authorization: `Bearer ${token}` } : {}) } });
  const body = (await res.json().catch(() => null)) as T | ApiError | null;
  if (res.status === 401) throw new Unauthenticated((body as ApiError | null)?.error?.message ?? "An API token is required.");
  if (!res.ok || body === null) throw new Error((body as ApiError | null)?.error?.message ?? `HTTP ${res.status}`);
  return body as T;
}

function load<T>(path: string, set: (l: Load<T>) => void, onAuth: (msg: string) => void) {
  set({ kind: "loading" });
  getJson<T>(path)
    .then((data) => set({ kind: "ok", data }))
    .catch((e: Error) => { if (e instanceof Unauthenticated) onAuth(e.message); set({ kind: "error", message: e.message }); });
}

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

function StatusRow({ label, tone, value }: { label: string; tone: "ok" | "off" | "pending"; value: string }) {
  const dot = tone === "ok" ? "status-success" : tone === "off" ? "status-error" : "status-neutral";
  return (
    <div className="flex items-center justify-between gap-4 px-4 py-3">
      <span className="text-sm font-medium">{label}</span>
      <span className="flex items-center gap-2 text-sm text-base-content/70">
        <span className={`status ${dot}`} aria-hidden="true" />
        {value}
      </span>
    </div>
  );
}

function SourceNote({ source }: { source: Source }) {
  const time = new Date(source.observedAt).toLocaleTimeString();
  return source.provider === "mock" ? (
    <span className="badge badge-sm badge-ghost" data-testid="source-note">Mock data · not a real cluster</span>
  ) : (
    <span className="text-xs text-base-content/60" data-testid="source-note">From Incus · observed {time}</span>
  );
}

function Section<T>({ title, state, testId, children }: { title: string; state: Load<T>; testId: string; children: (data: T) => React.ReactNode }) {
  return (
    <section className="flex flex-col gap-2" aria-label={title} data-testid={testId}>
      {state.kind === "loading" && <h2 className="font-semibold">{title}</h2>}
      {state.kind === "error" && (
        <>
          <h2 className="font-semibold">{title}</h2>
          <p role="alert" className="rounded-box border border-error/40 px-4 py-3 text-sm text-error" data-testid={`${testId}-error`}>{state.message}</p>
        </>
      )}
      {state.kind === "ok" && children(state.data)}
    </section>
  );
}

const KIND_LABEL: Record<Instance["kind"], string> = { container: "Container", "virtual-machine": "Virtual machine", unknown: "Unknown" };

export function App() {
  const [api, setApi] = useState<ApiState>({ kind: "checking" });
  const [cluster, setCluster] = useState<Load<ClusterRes>>({ kind: "loading" });
  const [instances, setInstances] = useState<Load<ListRes<Instance>>>({ kind: "loading" });
  const [authNeeded, setAuthNeeded] = useState<string | null>(null);

  const check = useCallback(() => {
    setApi({ kind: "checking" });
    setAuthNeeded(null);
    getJson<Health>("/api/v1/health")
      .then((health) => {
        setApi({ kind: "up", health });
        const onAuth = (msg: string) => { writeToken(null); setAuthNeeded(msg); };
        load("/api/v1/cluster", setCluster, onAuth);
        load("/api/v1/instances", setInstances, onAuth);
      })
      .catch(() => setApi({ kind: "down" }));
  }, []);

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

  return (
    <div className="flex h-full flex-col overflow-auto bg-base-100 text-base-content">
      <header className="flex h-12 shrink-0 items-center gap-3 border-b border-base-300 px-4">
        <img src="/logo.svg" alt="" className="size-6" />
        <span className="text-sm font-semibold">Apelora Fabric Console</span>
        <span className="badge badge-sm badge-ghost" data-testid="mock-badge" title="Shows whether the data below comes from a real Incus cluster.">{badge}</span>
      </header>

      <main className="mx-auto flex w-full max-w-4xl flex-col gap-6 px-4 py-8">
        <h1 className="text-lg font-semibold">Cluster</h1>

        <section aria-label="Service status" aria-live="polite" className="divide-y divide-base-300 rounded-box border border-base-300" data-testid="status">
          <StatusRow label="Control plane" {...controlPlane} />
          <StatusRow label="Incus cluster" {...incus} />
          {up && <StatusRow label="Authentication" tone={up.auth === "tokens" ? "ok" : "off"} value={up.auth === "tokens" ? "API tokens required" : "Disabled (development only)"} />}
        </section>

        {authNeeded && <SignIn message={authNeeded} onSubmit={(t) => { writeToken(t); check(); }} />}

        {!incusConnected && (
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
                The tables below show mock data. Set <code className="rounded bg-base-200 px-1 py-0.5 text-xs">APELORA_INCUS_URL</code> and the certificate paths for the control plane to read a real cluster.
              </p>
            )}
            <button type="button" className={`btn btn-sm ${authNeeded ? "btn-outline" : "btn-primary"}`} onClick={check} disabled={api.kind === "checking"}>
              Check again
            </button>
          </section>
        )}

        {up && !authNeeded && (
          <Section title="Cluster members" state={cluster} testId="members">
            {(c) => (
              <>
                <div className="flex items-center justify-between gap-3">
                  <h2 className="font-semibold">Cluster members <span className="font-normal text-base-content/60">({c.members.length})</span></h2>
                  <SourceNote source={c.source} />
                </div>
                {c.members.length === 0 ? (
                  <p className="text-sm text-base-content/70">{c.server.clustered ? "The cluster reports no members." : "This Incus server is not clustered."}</p>
                ) : (
                  <div className="overflow-x-auto rounded-box border border-base-300">
                    <table className="table table-sm">
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
                    </table>
                  </div>
                )}
              </>
            )}
          </Section>
        )}

        {up && !authNeeded && (
          <Section title="Instances" state={instances} testId="instances">
            {(l) => (
              <>
                <div className="flex items-center justify-between gap-3">
                  <h2 className="font-semibold">Instances <span className="font-normal text-base-content/60">({l.items.length})</span></h2>
                  <SourceNote source={l.source} />
                </div>
                {l.items.length === 0 ? (
                  <p className="text-sm text-base-content/70">No instances yet.</p>
                ) : (
                  <div className="overflow-x-auto rounded-box border border-base-300">
                    <table className="table table-sm">
                      <thead><tr><th>Instance</th><th>Kind</th><th>Status</th><th>Member</th><th>Project</th></tr></thead>
                      <tbody>
                        {l.items.map((i) => (
                          <tr key={`${i.project}/${i.name}`} data-testid="instance-row">
                            <td className="font-medium">{i.name}</td>
                            <td>{KIND_LABEL[i.kind]}</td>
                            <td>{i.status}</td>
                            <td>{i.location || "—"}</td>
                            <td>{i.project}</td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </div>
                )}
              </>
            )}
          </Section>
        )}
      </main>
    </div>
  );
}
