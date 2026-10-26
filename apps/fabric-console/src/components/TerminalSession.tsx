// SPDX-License-Identifier: AGPL-3.0-only
import { useCallback, useEffect, useId, useRef, useState } from "react";

import { ApiRequestError, type AccessList, type ApprovalDto, type KindAccess, type SessionTicket } from "@apelora/api-client";
import { KIND_ACTION, KIND_LABEL, approvalTarget, permissionOf, socketUrl, type Kind } from "@apelora/terminal";
import { TerminalScreen } from "@apelora/terminal/screen";

import { client } from "../lib/api";
import { actorRef, describeError, fmtTime } from "../lib/identity";
import type { TerminalTarget } from "./TerminalDialog";
import { Badge, ErrorPanel, Notice, sentence, type Tone } from "./ui";

type Phase = "prepare" | "connecting" | "live" | "ended" | "handover";

/** What the control plane says about this instance, read when the dialog opens. */
type Access =
  | { kind: "loading" }
  | { kind: "error"; message: string }
  | { kind: "unobserved" }
  | { kind: "absent" }
  | { kind: "ok"; kinds: KindAccess[]; idleSecs: number; maxSecs: number; mock: boolean };

/** What the dialog offers first: a terminal for Linux modules, PowerShell for Windows ones. */
const PREFERRED: Kind[] = ["terminal", "powershell", "rdp"];

const APPROVAL: Record<string, { tone: Tone; label: string }> = {
  pending: { tone: "warn", label: "Waiting for approval" },
  approved: { tone: "ok", label: "Approved" },
  rejected: { tone: "error", label: "Rejected" },
  cancelled: { tone: "neutral", label: "Cancelled" },
  consumed: { tone: "neutral", label: "Used" },
};
const POLL_MS = 5_000;

/**
 * The caller's newest request for this session; only pending and approved ones can still be used.
 * People who may approve also see others' open requests, and an approval works only for the person
 * who asked, so the requester must be the caller.
 */
function latestRequest(items: ApprovalDto[], action: string, target: string, requester: string): ApprovalDto | null {
  const mine = items.filter((a) => a.action === action && a.target === target && a.requester === requester).sort((a, b) => b.createdAt.localeCompare(a.createdAt));
  return mine[0] ?? null;
}
const usable = (a: ApprovalDto | null, now = Date.now()) => !!a && (a.state === "pending" || a.state === "approved") && Date.parse(a.expiresAt) > now;

/**
 * Production: another person approves the session first. Requests one, shows its state as it
 * changes, and hands the approved id to the dialog. Uses the same approvals API as the
 * Approvals page.
 */
function ApprovalStep({ target, kind, onAuth, onApproved }: { target: TerminalTarget; kind: Kind; onAuth: (m: string) => void; onApproved: (id: string | null) => void }) {
  const action = permissionOf(kind);
  const on = approvalTarget(target.application, target.instance);
  const [request, setRequest] = useState<ApprovalDto | null | undefined>(undefined);
  const [reason, setReason] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const reasonId = useId();
  const fail = useCallback((e: unknown) => {
    if (e instanceof ApiRequestError && e.unauthenticated) onAuth(e.message);
    setError(describeError(e));
  }, [onAuth]);

  // Who the caller is, as approvals name requesters (asked once).
  const self = useRef<Promise<string> | null>(null);
  const check = useCallback(async () => {
    try {
      self.current ??= client.get("whoami").then(actorRef);
      const [{ items }, me] = await Promise.all([client.get("approvals"), self.current]);
      setRequest(latestRequest(items, action, on, me));
      setError(null);
    } catch (e) {
      self.current = null;
      fail(e);
      setRequest((r) => r ?? null);
    }
  }, [action, on, fail]);

  useEffect(() => { void check(); }, [check]);
  // A pending request is checked again while the page is visible, so approval shows up on its own.
  const pending = request?.state === "pending";
  useEffect(() => {
    if (!pending) return;
    const id = setInterval(() => { if (document.visibilityState === "visible") void check(); }, POLL_MS);
    return () => clearInterval(id);
  }, [pending, check]);

  const live = usable(request ?? null) ? request! : null;
  useEffect(() => { onApproved(live?.state === "approved" ? live.id : null); }, [live?.state, live?.id, onApproved]);

  const ask = async () => {
    setBusy(true);
    setError(null);
    try {
      setRequest(await client.request("request_approval", { body: { action, target: on, reason: reason.trim() } }));
    } catch (e) {
      fail(e);
    } finally {
      setBusy(false);
    }
  };

  const state = request ? APPROVAL[request.state] ?? { tone: "neutral" as Tone, label: request.state } : null;
  return (
    <fieldset className="flex flex-col gap-3 rounded-box border border-warning/50 p-3" data-testid="session-approval">
      <legend className="px-1 text-sm font-medium">Approval needed</legend>
      <p className="text-sm">{target.application} runs in production, so another administrator of its project or organization approves first.</p>
      {request === undefined && <p role="status" className="text-sm text-base-content/60">Checking your requests…</p>}
      {request && state && (
        <div className="flex flex-col gap-1 text-sm" data-testid="approval-state" data-state={request.state}>
          <span className="flex flex-wrap items-center gap-2"><Badge tone={state.tone}>{state.label}</Badge><span className="text-base-content/70">requested {fmtTime(request.createdAt)}</span></span>
          {request.state === "approved" && usable(request) && <span>Approved. Open the terminal before {fmtTime(request.expiresAt)}.</span>}
          {request.state === "rejected" && <span>It was rejected. You can ask again with more detail.</span>}
          {!usable(request) && request.state !== "rejected" && <span>It can no longer be used. Ask again to open a new session.</span>}
        </div>
      )}
      {request !== undefined && !live && (
        <div className="flex flex-col gap-2 sm:flex-row sm:items-end">
          <div className="flex flex-1 flex-col gap-1 text-sm">
            <label htmlFor={reasonId}>Why do you need access?</label>
            <input id={reasonId} className="input input-sm w-full" value={reason} maxLength={300} onChange={(e) => setReason(e.target.value)} autoComplete="off" data-testid="approval-reason" />
          </div>
          <button type="button" className="btn btn-sm btn-outline" disabled={busy || reason.trim().length < 3} onClick={() => void ask()} data-testid="approval-request">{busy ? "Requesting…" : "Request approval"}</button>
        </div>
      )}
      {pending && <button type="button" className="btn btn-xs btn-ghost self-start" onClick={() => void check()} data-testid="approval-check">Check now</button>}
      {error && <ErrorPanel message={error} testId="approval-error" />}
    </fieldset>
  );
}

/** Copies text to the clipboard; says so for a moment. */
function CopyButton({ text, label, testId }: { text: string; label: string; testId: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <button
      type="button"
      className="btn btn-xs btn-outline"
      onClick={() => { void navigator.clipboard?.writeText(text).then(() => { setCopied(true); setTimeout(() => setCopied(false), 1500); }); }}
      data-testid={testId}
    >
      {copied ? "Copied" : label}
    </button>
  );
}

/**
 * A remote desktop is opened by the person's own RDP client through `apelora-desktop-tunnel` on
 * their computer: the browser cannot listen on a local port, and the module's RDP port is never
 * published. Shows the command for this control plane and page, and the single-use ticket the
 * tunnel asks for.
 */
function DesktopHandover({ ticket }: { ticket: SessionTicket }) {
  const command = `apelora-desktop-tunnel --url ${socketUrl(ticket)} --origin ${location.origin}`;
  // Once the ticket can no longer be used it leaves the page, so it is not left on screen.
  const [expired, setExpired] = useState(() => Date.parse(ticket.expiresAt) <= Date.now());
  useEffect(() => {
    if (expired) return;
    const id = setTimeout(() => setExpired(true), Math.max(0, Date.parse(ticket.expiresAt) - Date.now()));
    return () => clearTimeout(id);
  }, [ticket.expiresAt, expired]);
  return (
    <div className="flex flex-col gap-3 text-sm" data-testid="desktop-handover">
      <ol className="list-decimal space-y-3 pl-5">
        <li className="flex flex-col gap-1">
          <span>On your computer, run:</span>
          <code className="block overflow-x-auto rounded-box bg-base-200 px-3 py-2 font-mono text-xs" data-testid="desktop-command">{command}</code>
          <span><CopyButton text={command} label="Copy command" testId="desktop-copy-command" /></span>
        </li>
        {expired ? (
          <li data-testid="desktop-expired">The ticket has expired and was removed from this page. If the tunnel has not used it, close this window and ask for a new one.</li>
        ) : (
          <li className="flex flex-col gap-1">
            <span>When it asks, paste this ticket. It works once, until {fmtTime(ticket.expiresAt)}. Do not keep it in a clipboard that syncs to other devices.</span>
            <code className="block overflow-x-auto rounded-box bg-base-200 px-3 py-2 font-mono text-xs" data-testid="desktop-ticket">{ticket.ticket}</code>
            <span><CopyButton text={ticket.ticket} label="Copy ticket" testId="desktop-copy-ticket" /></span>
          </li>
        )}
        <li>Connect your remote desktop client to the address the tunnel prints (on <span className="font-mono">127.0.0.1</span>), check the certificate Windows presents, and sign in.</li>
      </ol>
      <p className="text-xs text-base-content/60">Ends after {Math.round(ticket.idleSecs / 60)} min without input, and at {Math.round(ticket.maxSecs / 60)} min. You can end it from Sessions.</p>
    </div>
  );
}

/** The dialog itself (loaded with the terminal code; see `TerminalDialog`). */
export function TerminalSession({ target, onClose, onAuth }: { target: TerminalTarget; onClose: () => void; onAuth: (message: string) => void }) {
  const [access, setAccess] = useState<Access>({ kind: "loading" });
  const [attempt, setAttempt] = useState(0);
  const [phase, setPhase] = useState<Phase>("prepare");
  const [ticket, setTicket] = useState<SessionTicket | null>(null);
  const [approval, setApproval] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [ended, setEnded] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const dialog = useRef<HTMLDialogElement>(null);
  const endSession = useRef<(() => void) | null>(null);
  const titleId = useId();

  useEffect(() => { if (!dialog.current?.open) dialog.current?.showModal(); }, []);

  useEffect(() => {
    let current = true;
    setAccess({ kind: "loading" });
    client.request("application_access", { params: { name: target.application } }).then(
      (a: AccessList) => {
        if (!current) return;
        if (!a.observed) { setAccess({ kind: "unobserved" }); return; }
        const item = a.items.find((i) => i.instance === target.instance);
        const kinds = PREFERRED.map((k) => item?.kinds.find((x) => x.kind === k)).filter((x): x is KindAccess => !!x);
        setAccess(kinds.length > 0 ? { kind: "ok", kinds, idleSecs: a.idleSecs, maxSecs: a.maxSecs, mock: a.provider === "mock" } : { kind: "absent" });
      },
      (e: unknown) => {
        if (!current) return;
        if (e instanceof ApiRequestError && e.unauthenticated) onAuth(e.message);
        setAccess({ kind: "error", message: describeError(e) });
      },
    );
    return () => { current = false; };
  }, [target, onAuth, attempt]);

  const [chosen, setChosen] = useState<Kind | null>(null);
  const loaded = access.kind === "ok" ? access : null;
  // The person's choice, else the first kind they may open, else the first offered.
  const picked = loaded && (loaded.kinds.find((k) => k.kind === chosen) ?? loaded.kinds.find((k) => k.allowed) ?? loaded.kinds[0]);
  const ok = loaded && picked ? { ...loaded, access: picked } : null;
  const kind: Kind = ok?.access.kind ?? "terminal";
  const needsApproval = !!ok?.access.allowed && ok.access.approvalRequired;

  const begin = async () => {
    setBusy(true);
    setError(null);
    try {
      setTicket(await client.request("open_session", { params: { name: target.application, instance: target.instance }, body: { kind, cols: 100, rows: 30 }, approvalId: approval ?? undefined }));
      // A remote desktop is opened by the person's own client through the tunnel, not here.
      setPhase(kind === "rdp" ? "handover" : "connecting");
    } catch (e) {
      if (e instanceof ApiRequestError && e.unauthenticated) onAuth(e.message);
      setError(describeError(e));
    } finally {
      setBusy(false);
    }
  };

  const connected = phase === "connecting" || phase === "live";
  const end = () => { endSession.current?.(); };
  const mock = ticket ? ticket.provider === "mock" : !!ok?.mock;

  return (
    <dialog
      ref={dialog}
      className="modal"
      aria-labelledby={titleId}
      data-testid="session-dialog"
      // Escape belongs to the program in the terminal; the buttons end the session.
      onCancel={(e) => { e.preventDefault(); if (!connected) onClose(); }}
    >
      <div className="modal-box flex max-h-[90vh] w-full max-w-4xl flex-col gap-4">
        <header className="flex flex-wrap items-center gap-2">
          <h2 id={titleId} className="font-semibold">{KIND_LABEL[kind]} · <span className="font-mono text-sm">{target.instance}</span></h2>
          {mock && <Badge tone="warn" testId="session-mock">Mock · no real instance</Badge>}
          {phase === "live" && <Badge tone="ok" testId="session-live">Connected</Badge>}
          {phase === "connecting" && <span className="badge badge-sm badge-ghost" role="status">Connecting…</span>}
        </header>

        {phase === "prepare" && (
          <div className="flex flex-col gap-3 text-sm" data-testid="session-prepare">
            <p className="text-base-content/80">Application <span className="font-medium text-base-content">{target.application}</span></p>
            {access.kind === "loading" && <p role="status" className="text-base-content/60">Checking what you may open…</p>}
            {access.kind === "error" && <ErrorPanel message={access.message} testId="session-access-error" onRetry={() => setAttempt((n) => n + 1)} />}
            {access.kind === "unobserved" && <Notice tone="warn" testId="session-unavailable">The control plane cannot read Incus right now, so it cannot tell whether the instance is running. Try again in a moment.</Notice>}
            {access.kind === "absent" && <Notice tone="warn" testId="session-unavailable">{target.instance} is not a running instance of {target.application} that Apelora can open a session into.</Notice>}
            {ok && !ok.access.allowed && <Notice tone="warn" testId="session-unavailable" title="Not available">{sentence(ok.access.reason ?? "This session is not available.")}.</Notice>}
            {loaded && loaded.kinds.length > 1 && (
              <fieldset className="flex flex-wrap gap-4" data-testid="session-kinds">
                <legend className="sr-only">What to open</legend>
                {loaded.kinds.map((k) => (
                  <label key={k.kind} className="flex items-center gap-2">
                    <input type="radio" className="radio radio-sm" name="session-kind" checked={k.kind === kind} onChange={() => { setChosen(k.kind); setApproval(null); }} data-testid={`session-kind-${k.kind}`} />
                    {KIND_LABEL[k.kind]}
                  </label>
                ))}
              </fieldset>
            )}
            {ok?.access.allowed && (
              <ul className="list-disc space-y-1 pl-5 text-base-content/80">
                {kind === "rdp"
                  ? <li>Your own remote desktop client connects through the control plane; the module's RDP port is never published. Apelora relays the encrypted connection without decrypting it; you sign in to Windows yourself.</li>
                  : <li>Runs as an unprivileged user, through the control plane.</li>}
                <li>Ends after {Math.round(ok.idleSecs / 60)} minutes without {kind === "rdp" ? "input" : "typing"}, or at {Math.round(ok.maxSecs / 60)} minutes.</li>
                <li>Who opened it and why it ended are recorded; what you type or see is not.</li>
              </ul>
            )}
            {needsApproval && <ApprovalStep key={kind} target={target} kind={kind} onAuth={onAuth} onApproved={setApproval} />}
          </div>
        )}

        {phase === "handover" && ticket && <DesktopHandover ticket={ticket} />}

        {phase !== "prepare" && phase !== "handover" && ticket && (
          <>
            <TerminalScreen
              ticket={ticket}
              label={`${KIND_LABEL[kind]} of ${target.instance}`}
              endRef={endSession}
              onLive={() => setPhase("live")}
              onEnded={(message) => { setEnded(message); setPhase("ended"); }}
              onError={setError}
            />
            {phase === "live" && <p className="text-xs text-base-content/60">Ends after {Math.round(ticket.idleSecs / 60)} min without typing, and at {Math.round(ticket.maxSecs / 60)} min. Not recorded.</p>}
            {ended && <p role="status" className="rounded-box border border-base-300 bg-base-200 px-3 py-2 text-sm" data-testid="session-ended">{ended}</p>}
          </>
        )}

        {error && <ErrorPanel message={error} testId="session-error" />}

        <div className="modal-action mt-0">
          {phase === "prepare" ? (
            <>
              <button type="button" className="btn btn-sm btn-ghost" onClick={onClose}>Cancel</button>
              {ok?.access.allowed && (
                <button type="button" className="btn btn-sm btn-primary" disabled={busy || (needsApproval && !approval)} onClick={() => void begin()} data-testid="session-open">
                  {busy ? "Opening…" : KIND_ACTION[kind]}
                </button>
              )}
            </>
          ) : phase === "handover" ? (
            <button type="button" className="btn btn-sm btn-primary" onClick={onClose} data-testid="session-close">Done</button>
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
