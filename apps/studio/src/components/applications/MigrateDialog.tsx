import { useEffect, useState } from "react";

import { type ClusterMemberDetail, type MigrationPreview } from "@apelora/api-client";

import { client, messageOf } from "../../features/deploy/api";
import { useLifecycle } from "../../features/applications/lifecycle";
import type { AppView } from "../../features/applications/model";
import { useAppToasts } from "../../features/applications/toast";

type Load<T> = { kind: "idle" } | { kind: "loading" } | { kind: "error"; message: string } | { kind: "ready"; data: T };

/**
 * Migrate: choose the server, read what would happen to each instance (the control plane's dry
 * run), and only then ask for it. What has to stop while it moves needs a tick, because its clients
 * lose service meanwhile. Nothing moves until the control plane has checked it again.
 */
export function MigrateDialog({ app, onClose }: { app: AppView; onClose: () => void }) {
  const run = useLifecycle((s) => s.run);
  const toast = useAppToasts((s) => s.push);
  const [members, setMembers] = useState<Load<ClusterMemberDetail[]>>({ kind: "loading" });
  const [target, setTarget] = useState("");
  const [typed, setTyped] = useState("");
  const [preview, setPreview] = useState<Load<MigrationPreview>>({ kind: "idle" });
  const [agree, setAgree] = useState(false);
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const chosen = members.kind === "ready" && members.data.length > 0 ? target : typed.trim();

  useEffect(() => {
    let live = true;
    client.request("cluster_members", {}).then(
      (r) => { if (live) setMembers({ kind: "ready", data: r.members }); },
      (e: unknown) => { if (live) setMembers({ kind: "error", message: messageOf(e) }); },
    );
    return () => { live = false; };
  }, []);
  useEffect(() => {
    const on = (e: KeyboardEvent) => { if (e.key === "Escape") onClose(); };
    window.addEventListener("keydown", on);
    return () => window.removeEventListener("keydown", on);
  }, [onClose]);
  useEffect(() => {
    setAgree(false);
    setError(null);
    if (!chosen) { setPreview({ kind: "idle" }); return; }
    let live = true;
    setPreview({ kind: "loading" });
    const t = setTimeout(() => {
      client.request("preview_application_migration", { params: { name: app.name }, query: { target: chosen } }).then(
        (r) => { if (live) setPreview({ kind: "ready", data: r }); },
        (e: unknown) => { if (live) setPreview({ kind: "error", message: messageOf(e) }); },
      );
    }, 250);
    return () => { live = false; clearTimeout(t); };
  }, [chosen, app.name]);

  const p = preview.kind === "ready" ? preview.data : null;
  const stops = p?.stops ?? [];
  const ready = !!p && p.possible && !sending && (stops.length === 0 || agree);
  const go = async () => {
    if (!ready) return;
    setSending(true);
    setError(null);
    const r = await run(app.name, "migrate", stops.length > 0, chosen);
    if (r.ok) { toast("info", `Migrating ${app.name} to ${chosen}. Each instance is confirmed on the new server before it is reported moved.`); onClose(); return; }
    setSending(false);
    setError(r.message);
  };

  const how = (i: MigrationPreview["items"][number]) => i.alreadyThere ? "Already there" : i.blocked.length ? "Cannot move" : i.live ? "Keeps running" : i.stops ? "Stops while it moves" : "Moves (stopped)";
  const servers = members.kind === "ready" ? members.data : [];
  return (
    <dialog className="modal modal-open" aria-labelledby="mg-title" data-testid="migrate-dialog">
      <div className="modal-box flex max-w-2xl flex-col gap-3">
        <h3 id="mg-title" className="text-base font-semibold">Migrate {app.name}</h3>
        <p className="text-sm text-base-content/80">Moves every instance of this application to one server. What can keep running while it moves does; the rest stops, moves, and starts again. Data and volumes go with it.</p>
        {members.kind === "loading" && <p className="text-sm text-base-content/70">Reading the servers…</p>}
        {members.kind === "error" && (
          <label className="flex flex-col gap-1 text-sm">
            <span>Server to move to</span>
            <input className="input input-sm font-mono" value={typed} onChange={(e) => setTyped(e.target.value)} placeholder="Name of the server" data-testid="migrate-target-input" />
            <span className="text-xs text-base-content/60" role="status">The servers could not be listed ({members.message}). Type the name of one.</span>
          </label>
        )}
        {members.kind === "ready" && (
          servers.length === 0
            ? <p className="text-sm text-base-content/70" data-testid="migrate-no-servers">This installation has one server and nothing to move to.</p>
            : (
              <label className="flex flex-col gap-1 text-sm">
                <span>Server to move to</span>
                <select className="select select-sm" value={target} onChange={(e) => setTarget(e.target.value)} data-testid="migrate-target">
                  <option value="">Choose a server…</option>
                  {servers.map((m) => <option key={m.name} value={m.name} disabled={m.status !== "Online"}>{m.name}{m.status !== "Online" ? ` (${m.status})` : ""}</option>)}
                </select>
              </label>
            )
        )}
        {preview.kind === "loading" && <p className="text-sm text-base-content/70" role="status">Checking what would happen…</p>}
        {preview.kind === "error" && <p role="alert" className="text-sm text-error" data-testid="migrate-error">{preview.message}</p>}
        {p && (
          <>
            <div className="max-h-60 overflow-auto rounded-box border border-base-300">
              <table className="table table-sm" data-testid="migrate-plan">
                <thead><tr><th>Instance</th><th>Now on</th><th>What happens</th></tr></thead>
                <tbody>
                  {p.items.map((i) => (
                    <tr key={i.instance} data-testid="migrate-row" data-how={how(i)}>
                      <td className="font-mono text-xs">{i.instance}</td>
                      <td>{i.from || "—"}</td>
                      <td>
                        {how(i)}
                        {i.blocked.map((b) => <span key={b} className="block text-xs text-error">{b}</span>)}
                        {i.notes.map((n) => <span key={n} className="block text-xs text-base-content/60">{n}</span>)}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
            <p className="text-xs text-base-content/60">All instances end up on {p.target}. Replicas of one module then share that server, so they no longer survive its failure separately.</p>
            {!p.possible && !p.items.some((i) => i.blocked.length) && <p className="text-sm" role="status" data-testid="migrate-nothing">Every instance already runs on {p.target}.</p>}
            {stops.length > 0 && p.possible && (
              <label className="flex items-start gap-2 text-sm">
                <input type="checkbox" className="checkbox checkbox-sm mt-0.5" checked={agree} onChange={(e) => setAgree(e.target.checked)} data-testid="migrate-agree" />
                <span>{stops.join(", ")} will stop while {stops.length === 1 ? "it moves" : "they move"}: clients lose service meanwhile.</span>
              </label>
            )}
          </>
        )}
        {error && <p role="alert" className="text-sm text-error" data-testid="migrate-start-error">{error}</p>}
        <div className="modal-action mt-0">
          <button type="button" className="btn btn-sm" onClick={onClose}>Cancel</button>
          <button type="button" className="btn btn-sm btn-primary" disabled={!ready} onClick={() => void go()} data-testid="migrate-confirm">{sending ? "Starting…" : "Migrate"}</button>
        </div>
      </div>
      <div className="modal-backdrop" onClick={onClose} />
    </dialog>
  );
}
