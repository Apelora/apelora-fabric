import { useCallback, useEffect, useState } from "react";

/** Shape of `GET /api/v1/health` from the Apelora control plane. */
type Health = {
  status: string;
  version: string;
  provider: "mock" | "incus";
  incus: { connected: boolean; apiVersion?: string; serverVersion?: string | null; error?: string };
};

type ApiState = { kind: "checking" } | { kind: "up"; health: Health } | { kind: "down" };

async function fetchHealth(): Promise<Health> {
  const res = await fetch("/api/v1/health", { headers: { accept: "application/json" } });
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  return (await res.json()) as Health;
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

export function App() {
  const [api, setApi] = useState<ApiState>({ kind: "checking" });

  const check = useCallback(() => {
    setApi({ kind: "checking" });
    fetchHealth()
      .then((health) => setApi({ kind: "up", health }))
      .catch(() => setApi({ kind: "down" }));
  }, []);

  useEffect(check, [check]);

  const controlPlane =
    api.kind === "checking"
      ? { tone: "pending" as const, value: "Checking…" }
      : api.kind === "down"
        ? { tone: "off" as const, value: "Not reachable" }
        : { tone: "ok" as const, value: `Running · v${api.health.version} · ${api.health.provider === "mock" ? "mock data" : "Incus"}` };

  const incusConnected = api.kind === "up" && api.health.incus.connected;
  const incus =
    api.kind === "up"
      ? incusConnected
        ? { tone: "ok" as const, value: "Connected" }
        : { tone: "off" as const, value: "Not connected" }
      : { tone: "pending" as const, value: "Unknown" };

  return (
    <div className="flex h-full flex-col bg-base-100 text-base-content">
      <header className="flex h-12 shrink-0 items-center gap-3 border-b border-base-300 px-4">
        <img src="/logo.svg" alt="" className="size-6" />
        <span className="text-sm font-semibold">Apelora Fabric Console</span>
        <span className="badge badge-sm badge-ghost" data-testid="mock-badge" title="No cluster is connected; nothing here is live infrastructure.">
          {incusConnected ? "Connected" : "Mock · not connected"}
        </span>
      </header>

      <main className="mx-auto flex w-full max-w-2xl flex-col gap-6 px-4 py-8">
        <h1 className="text-lg font-semibold">Cluster</h1>

        <section aria-label="Service status" aria-live="polite" className="divide-y divide-base-300 rounded-box border border-base-300" data-testid="status">
          <StatusRow label="Control plane" {...controlPlane} />
          <StatusRow label="Incus cluster" {...incus} />
        </section>

        {!incusConnected && (
          <section className="flex flex-col items-start gap-3 rounded-box border border-dashed border-base-300 p-6" data-testid="empty-state">
            <h2 className="font-semibold">No cluster connected yet</h2>
            <p className="text-sm text-base-content/70">
              Members, instances, networks and storage appear here once the control plane is connected to an Incus cluster.
            </p>
            {api.kind === "down" ? (
              <p className="text-sm text-base-content/70">
                Start the control plane with <code className="rounded bg-base-200 px-1 py-0.5 text-xs">cargo run -p apelora-control-plane</code>, then check again.
              </p>
            ) : (
              <p className="text-sm text-base-content/70">Connecting to a real Incus cluster arrives in a later development phase.</p>
            )}
            <button type="button" className="btn btn-sm btn-primary" onClick={check} disabled={api.kind === "checking"}>
              Check again
            </button>
          </section>
        )}
      </main>
    </div>
  );
}
