import { useCallback, useEffect, useRef, useState } from "react";
import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import type { SessionTicket } from "@apelora/api-client";

import { messageOf } from "../../features/deploy/api";
import { KIND_ACTION, KIND_LABEL, endedBecause, openSession, openSocket, requestApproval, useSessions, type Target } from "../../features/sessions/sessions";

type Phase = "prepare" | "connecting" | "live" | "ended";

const THEME = { background: "#1b1f24", foreground: "#e6e8eb", cursor: "#e6e8eb", selectionBackground: "#3b4452" };

function Session({ target, onClose }: { target: Target; onClose: () => void }) {
  const [phase, setPhase] = useState<Phase>("prepare");
  const [ticket, setTicket] = useState<SessionTicket | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [reason, setReason] = useState("");
  const [approval, setApproval] = useState("");
  const [asked, setAsked] = useState(false);
  const [ended, setEnded] = useState<string | null>(null);
  const host = useRef<HTMLDivElement>(null);
  const socket = useRef<WebSocket | null>(null);
  const dialog = useRef<HTMLDialogElement>(null);

  useEffect(() => { dialog.current?.showModal(); }, []);

  const begin = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      setTicket(await openSession(target, { cols: 100, rows: 30 }, approval.trim() || undefined));
      setPhase("connecting");
    } catch (e) {
      setError(messageOf(e));
    } finally {
      setBusy(false);
    }
  }, [target, approval]);

  const ask = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      const id = await requestApproval(target, reason.trim());
      setApproval(id);
      setAsked(true);
    } catch (e) {
      setError(messageOf(e));
    } finally {
      setBusy(false);
    }
  }, [target, reason]);

  useEffect(() => {
    if (!ticket || !host.current) return;
    const term = new Terminal({ cursorBlink: true, fontSize: 13, scrollback: 2000, theme: THEME, fontFamily: "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace" });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host.current);
    fit.fit();
    const enc = new TextEncoder();
    const ws = openSocket(ticket);
    ws.binaryType = "arraybuffer";
    socket.current = ws;
    let finished = false;
    const finish = (why: string) => {
      if (finished) return;
      finished = true;
      setEnded(endedBecause(why, ticket.idleSecs, ticket.maxSecs));
      setPhase("ended");
      term.options.cursorBlink = false;
    };
    ws.onopen = () => {
      setPhase("live");
      ws.send(JSON.stringify({ type: "resize", cols: term.cols, rows: term.rows }));
      term.focus();
    };
    ws.onmessage = (ev: MessageEvent) => {
      if (typeof ev.data === "string") {
        try {
          const m = JSON.parse(ev.data) as { type?: string; reason?: string; message?: string };
          if (m.type === "closed") finish(m.reason ?? "");
          else if (m.type === "error") setError(m.message ?? "The session could not be opened.");
        } catch { /* not ours */ }
        return;
      }
      term.write(new Uint8Array(ev.data as ArrayBuffer));
    };
    ws.onclose = () => finish("client-closed");
    ws.onerror = () => setError("The connection to the control plane failed.");
    const typed = term.onData((d) => { if (ws.readyState === WebSocket.OPEN) ws.send(enc.encode(d)); });
    const sized = term.onResize(({ cols, rows }) => { if (ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify({ type: "resize", cols, rows })); });
    const watch = new ResizeObserver(() => { try { fit.fit(); } catch { /* hidden */ } });
    watch.observe(host.current);
    return () => {
      finished = true;
      watch.disconnect();
      typed.dispose();
      sized.dispose();
      ws.close();
      socket.current = null;
      term.dispose();
    };
  }, [ticket]);

  const connected = phase === "connecting" || phase === "live";
  const end = () => { socket.current?.close(); if (!connected) onClose(); };
  const mock = ticket?.provider === "mock";

  return (
    <dialog
      ref={dialog}
      className="modal"
      aria-labelledby="session-title"
      data-testid="session-dialog"
      // Escape belongs to the program in the terminal; the buttons end the session.
      onCancel={(e) => { if (connected) e.preventDefault(); else onClose(); }}
    >
      <div className="modal-box flex max-h-[90vh] w-full max-w-4xl flex-col gap-3">
        <header className="flex flex-wrap items-center gap-2">
          <h2 id="session-title" className="text-base font-semibold">{KIND_LABEL[target.kind]} · <span className="font-mono text-sm">{target.instance}</span></h2>
          {mock && <span className="badge badge-warning badge-soft" data-testid="session-mock">Mock · no real instance</span>}
          {phase === "live" && <span className="badge badge-success badge-soft" data-testid="session-live">Connected</span>}
          {phase === "connecting" && <span className="badge badge-ghost" role="status">Connecting…</span>}
        </header>

        {phase === "prepare" && (
          <div className="flex flex-col gap-3 text-sm" data-testid="session-prepare">
            <ul className="list-disc space-y-1 pl-5 text-xs text-base-content/80">
              <li>The session runs as an unprivileged user inside the module, through the control plane. Nothing connects to the module directly.</li>
              <li>It ends after a period without typing, and at a time limit.</li>
              <li>Apelora records who opened it, when, and why it ended. It does not record what you type or see.</li>
            </ul>
            {target.approvalRequired && (
              <fieldset className="flex flex-col gap-2 rounded-box border border-warning/50 bg-warning/10 p-3" data-testid="session-approval">
                <legend className="px-1 text-xs font-medium">Another person must approve this first</legend>
                <p className="text-xs">{target.app} runs in a production environment. Ask an administrator or owner of its project or organisation to approve the request, then open the session with the approval.</p>
                {!asked ? (
                  <div className="flex flex-col gap-2 sm:flex-row sm:items-end">
                    <label className="form-control flex-1 text-xs">
                      <span className="mb-1 block">Why do you need access?</span>
                      <input className="input input-sm w-full" value={reason} maxLength={300} onChange={(e) => setReason(e.target.value)} data-testid="approval-reason" />
                    </label>
                    <button type="button" className="btn btn-sm" disabled={busy || reason.trim().length < 3} onClick={() => void ask()} data-testid="approval-request">Request approval</button>
                  </div>
                ) : (
                  <p className="text-xs" role="status" data-testid="approval-asked">Requested. Approval id <span className="font-mono">{approval}</span>. Once it is approved, open the session.</p>
                )}
                <label className="form-control text-xs">
                  <span className="mb-1 block">Approval id</span>
                  <input className="input input-sm w-full font-mono" value={approval} onChange={(e) => setApproval(e.target.value)} data-testid="approval-id" autoComplete="off" />
                </label>
              </fieldset>
            )}
          </div>
        )}

        {phase !== "prepare" && (
          <>
            <div ref={host} className="min-h-72 flex-1 overflow-hidden rounded-box bg-[#1b1f24] p-2" role="region" aria-label={`${KIND_LABEL[target.kind]} of ${target.instance}`} data-testid="session-terminal" />
            {ticket && phase === "live" && <p className="text-[11px] text-base-content/60">Ends after {Math.round(ticket.idleSecs / 60)} min without typing, and at {Math.round(ticket.maxSecs / 60)} min. Not recorded.</p>}
            {ended && <p role="status" className="rounded-box border border-base-300 bg-base-200 px-3 py-2 text-xs" data-testid="session-ended">{ended}</p>}
          </>
        )}

        {error && <p role="alert" className="rounded-box border border-error/50 bg-error/10 px-3 py-2 text-xs" data-testid="session-error">{error}</p>}

        <div className="modal-action mt-0">
          {phase === "prepare" ? (
            <>
              <button type="button" className="btn btn-sm" onClick={onClose}>Cancel</button>
              <button type="button" className="btn btn-sm btn-primary" disabled={busy || (target.approvalRequired && !approval.trim())} onClick={() => void begin()} data-testid="session-open">{KIND_ACTION[target.kind]}</button>
            </>
          ) : connected ? (
            <button type="button" className="btn btn-sm" onClick={end} data-testid="session-end">End session</button>
          ) : (
            <button type="button" className="btn btn-sm btn-primary" onClick={onClose} data-testid="session-close">Close</button>
          )}
        </div>
      </div>
    </dialog>
  );
}

/** Mounted once per application page: shows the session the person chose from a menu or a table. */
export function SessionDialog() {
  const target = useSessions((s) => s.target);
  const close = useSessions((s) => s.close);
  if (!target) return null;
  return <Session key={`${target.instance}:${target.kind}`} target={target} onClose={close} />;
}
