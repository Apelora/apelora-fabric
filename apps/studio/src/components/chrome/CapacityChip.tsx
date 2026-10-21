import { useCallback, useEffect, useId, useMemo, useRef, useState } from "react";
import { ApiRequestError, type CapacityReport } from "@apelora/api-client";
import { capacityDemand, capacityFit, deploymentModules, formatBytes, formatMemory, type CapacityFit, type FitVerdict } from "@apelora/assembly-model";
import { lookupClass } from "@apelora/module-catalog";

import { useBackend } from "../../features/applications/backend";
import { client, messageOf } from "../../features/deploy/api";
import { useStudio } from "../../features/editor/context";

/** How often the capacity is read again while the Studio is visible. */
const REFRESH_MS = 60_000;

type Load = { kind: "local" } | { kind: "loading" } | { kind: "error"; message: string; forbidden: boolean } | { kind: "ok"; report: CapacityReport };

const VERDICT: Record<FitVerdict, string> = {
  "one-server": "fits on one server",
  "across-servers": "fits across servers",
  "does-not-fit": "does not fit",
  "nothing-to-run": "nothing to run yet",
  unknown: "unknown",
};
const DOT: Record<FitVerdict, string> = { "one-server": "status-success", "across-servers": "status-success", "does-not-fit": "status-error", "nothing-to-run": "status-neutral", unknown: "status-neutral" };

/** Reads the cluster's free capacity while connected to a control plane. */
function useCapacity(connected: boolean): [Load, () => void] {
  const [load, setLoad] = useState<Load>(connected ? { kind: "loading" } : { kind: "local" });
  const read = useCallback(() => {
    if (!connected) { setLoad({ kind: "local" }); return; }
    client.get("capacity")
      .then((report) => setLoad({ kind: "ok", report }))
      .catch((e: unknown) => setLoad({ kind: "error", message: messageOf(e), forbidden: e instanceof ApiRequestError && e.status === 403 }));
  }, [connected]);
  useEffect(() => {
    read();
    if (!connected) return;
    const t = setInterval(() => { if (document.visibilityState === "visible") read(); }, REFRESH_MS);
    return () => clearInterval(t);
  }, [read, connected]);
  return [load, read];
}

/**
 * A small header chip: does the design fit the cluster's free CPU, memory and volume space?
 * Opens a panel with the numbers. Advisory only; the deployment decides.
 */
export function CapacityChip() {
  const connected = useBackend((s) => s.backend.kind === "api");
  // The library page does not look for a control plane on its own; opening the panel does.
  const detecting = useBackend((s) => s.status === "detecting");
  const detect = useBackend((s) => s.detect);
  const assembly = useStudio((s) => s.assembly);
  const [load, read] = useCapacity(connected);
  const [open, setOpen] = useState(false);
  const panelId = useId();
  const wrap = useRef<HTMLDivElement>(null);
  const button = useRef<HTMLButtonElement>(null);

  const split = useMemo(() => deploymentModules(assembly, lookupClass, "apelora"), [assembly]);
  const demand = useMemo(() => capacityDemand(split.deployable), [split]);
  const fit: CapacityFit | null = useMemo(() => {
    if (load.kind !== "ok") return null;
    const r = load.report;
    return capacityFit(demand, r.servers.map((s) => ({
      label: s.label,
      acceptsModules: s.acceptsModules,
      cpuThreads: s.cpuThreads ?? null,
      memoryFreeBytes: s.memory ? s.memory.totalBytes - s.memory.usedBytes : null,
      volumeFreeBytes: s.volumePool ? s.volumePool.totalBytes - s.volumePool.usedBytes : null,
    })), { volumePool: !!r.volumePool, volumePoolShared: r.volumePoolShared });
  }, [load, demand]);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") { e.stopPropagation(); setOpen(false); button.current?.focus(); } };
    const onDown = (e: PointerEvent) => { if (wrap.current && !wrap.current.contains(e.target as Node)) setOpen(false); };
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("pointerdown", onDown);
    return () => { window.removeEventListener("keydown", onKey, true); window.removeEventListener("pointerdown", onDown); };
  }, [open]);

  const mock = load.kind === "ok" && load.report.source.provider === "mock";
  const verdict: FitVerdict = fit?.verdict ?? "unknown";
  const label = load.kind === "loading" ? "Capacity…" : `${mock ? "Mock · " : ""}Capacity: ${fit ? VERDICT[verdict] : "unknown"}`;

  return (
    <div className="relative" ref={wrap}>
      <button ref={button} type="button" className="badge badge-sm badge-ghost min-h-6 gap-1.5 pointer-coarse:min-h-11" aria-expanded={open} aria-controls={panelId} data-testid="capacity-chip" data-verdict={fit?.verdict ?? load.kind}
        onClick={() => { if (!open) { if (detecting) void detect(); else read(); } setOpen(!open); }}>
        <span className={`status ${load.kind === "ok" ? DOT[verdict] : "status-neutral"}`} aria-hidden="true" />
        {label}
      </button>
      {open && (
        <div id={panelId} role="region" aria-label="Capacity for this design" className="absolute left-0 top-full z-40 mt-1 flex w-80 whitespace-normal max-w-[calc(100vw-1.5rem)] flex-col gap-3 rounded-box border border-base-300 bg-base-100 p-4 text-sm shadow-sm" data-testid="capacity-panel">
          <div className="flex flex-col gap-0.5">
            <h2 className="font-semibold">Capacity for this design</h2>
            {load.kind === "ok" && (mock
              ? <span className="badge badge-sm badge-ghost self-start whitespace-nowrap">Mock data</span>
              : <span className="whitespace-nowrap text-xs text-base-content/60">From Incus, {new Date(load.report.source.observedAt).toLocaleTimeString()}</span>)}
          </div>
          {load.kind === "local" && detecting && <p role="status">Looking for a control plane…</p>}
          {load.kind === "local" && !detecting && <p data-testid="capacity-local">Capacity unknown: this browser is not connected to a control plane. Connect to compare the design with the cluster.</p>}
          {load.kind === "loading" && <p role="status">Reading the cluster…</p>}
          {load.kind === "error" && (
            <div className="flex flex-col items-start gap-2" data-testid="capacity-error">
              <p>{load.forbidden ? "Your role does not include seeing the cluster's capacity." : `Could not read the capacity: ${load.message}`}</p>
              {!load.forbidden && <button type="button" className="btn btn-xs" onClick={read}>Try again</button>}
            </div>
          )}
          {load.kind === "ok" && fit && <Details report={load.report} fit={fit} demand={demand} designOnly={split.designOnly.length} />}
        </div>
      )}
    </div>
  );
}

function Details({ report, fit, demand, designOnly }: { report: CapacityReport; fit: CapacityFit; demand: ReturnType<typeof capacityDemand>; designOnly: number }) {
  const t = report.totals;
  const rows: [string, string, string][] = [
    ["vCPU", String(demand.cpu), `${t.cpuThreads} threads`],
    ["Memory", formatMemory(demand.memoryMiB), `${formatBytes(t.memoryFreeBytes)} of ${formatBytes(t.memoryTotalBytes)}`],
  ];
  if (demand.volumeMiB > 0 || t.volumePoolFreeBytes != null) {
    rows.push(["Volumes", demand.volumeMiB ? formatMemory(demand.volumeMiB) : "none", t.volumePoolFreeBytes != null ? formatBytes(t.volumePoolFreeBytes) : "no volume pool"]);
  }
  return (
    <div className="flex flex-col gap-3" data-testid="capacity-details">
      <div>
        <p className="font-medium" data-testid="capacity-verdict">{fit.verdict === "nothing-to-run" ? "Nothing in this design can be deployed yet." : `The design ${VERDICT[fit.verdict]}.`}</p>
        {[...fit.reasons, ...fit.notes].map((x) => <p key={x} className="text-base-content/70">{x}</p>)}
      </div>
      <table className="table table-xs">
        <thead><tr><th scope="col"><span className="sr-only">Resource</span></th><th scope="col" className="text-right">Asked for</th><th scope="col" className="text-right">Free</th></tr></thead>
        <tbody>
          {rows.map(([what, asked, free]) => (
            <tr key={what}><th scope="row" className="font-normal">{what}</th><td className="text-right tabular-nums">{asked}</td><td className="text-right tabular-nums">{free}</td></tr>
          ))}
        </tbody>
      </table>
      <div>
        <h3 className="text-xs font-semibold text-base-content/70">Servers ({t.serversAccepting} of {report.servers.length} take new modules)</h3>
        <ul className="mt-1 flex flex-col gap-0.5" data-testid="capacity-servers">
          {report.servers.map((s) => (
            <li key={s.label} className="flex items-baseline justify-between gap-3">
              <span className="min-w-0 truncate">{s.label}</span>
              <span className="shrink-0 text-right tabular-nums text-base-content/70">
                {s.acceptsModules && s.memory ? `${formatBytes(s.memory.totalBytes - s.memory.usedBytes)} free, ${s.cpuThreads ?? "?"} threads` : s.reason ?? "unknown"}
              </span>
            </li>
          ))}
        </ul>
      </div>
      <p className="text-xs text-base-content/60">
        Counts {demand.instances.length} instance{demand.instances.length === 1 ? "" : "s"} with replicas; not {designOnly ? `${designOnly} design-only module${designOnly === 1 ? "" : "s"} or ` : ""}root disks. Nothing is reserved until you deploy.
      </p>
    </div>
  );
}
