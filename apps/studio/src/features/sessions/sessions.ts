import { ApiRequestError, type AccessList, type InstanceAccess, type KindAccess, type SessionTicket } from "@apelora/api-client";
import { create } from "zustand";

import { client, messageOf } from "../deploy/api";

export type Kind = KindAccess["kind"];

export const KIND_LABEL: Record<Kind, string> = { terminal: "Terminal", powershell: "PowerShell", rdp: "Remote desktop" };
export const KIND_ACTION: Record<Kind, string> = { terminal: "Open terminal", powershell: "Open PowerShell", rdp: "Connect with RDP" };

/** What the person chose to open, and what the dialog needs to know about it. */
export interface Target {
  app: string;
  instance: string;
  appliance: string;
  kind: Kind;
  approvalRequired: boolean;
}

interface SessionsState {
  app: string | null;
  access: AccessList | null;
  error: string | null;
  target: Target | null;
  /** Follows the application on screen; `null` stops. Only a control plane can offer sessions. */
  watch: (app: string | null) => void;
  refresh: () => Promise<void>;
  open: (t: Target) => void;
  close: () => void;
}

const REFRESH_MS = 15_000;
let timer: ReturnType<typeof setInterval> | undefined;

export const useSessions = create<SessionsState>((set, get) => ({
  app: null,
  access: null,
  error: null,
  target: null,
  watch: (app) => {
    if (timer) clearInterval(timer);
    timer = undefined;
    set({ app, access: null, error: null });
    if (!app) return;
    void get().refresh();
    timer = setInterval(() => void get().refresh(), REFRESH_MS);
  },
  refresh: async () => {
    const app = get().app;
    if (!app) return;
    try {
      const access = await client.request("application_access", { params: { name: app } });
      if (get().app === app) set({ access, error: null });
    } catch (e) {
      if (get().app === app) set({ access: null, error: messageOf(e) });
    }
  },
  open: (target) => set({ target }),
  close: () => set({ target: null }),
}));

/** The instances of one appliance (its replicas) that offer sessions, in name order. */
export const instancesOf = (access: AccessList | null, appliance: string): InstanceAccess[] =>
  (access?.items ?? []).filter((i) => i.appliance === appliance);

export const kindOf = (i: InstanceAccess, kind: Kind): KindAccess | undefined => i.kinds.find((k) => k.kind === kind);

export interface Opened {
  ticket: SessionTicket;
}

/** Asks the control plane for a single-use ticket. `approvalId` is the approval production needs. */
export async function openSession(t: Target, size: { cols: number; rows: number }, approvalId?: string): Promise<SessionTicket> {
  return client.request("open_session", { params: { name: t.app, instance: t.instance }, body: { kind: t.kind, cols: size.cols, rows: size.rows }, approvalId: approvalId || undefined });
}

/** Asks another person to approve this instance's session. Returns the approval id. */
export async function requestApproval(t: Target, reason: string): Promise<string> {
  const a = await client.request("request_approval", { body: { action: t.kind === "rdp" ? "instance.desktop" : "instance.console", target: `instance:${t.app}/${t.instance}`, reason } });
  return a.id;
}

/** Opens the session's websocket. The ticket travels as a sub-protocol, never in the URL. */
export function openSocket(t: SessionTicket): WebSocket {
  const scheme = location.protocol === "https:" ? "wss" : "ws";
  return new WebSocket(`${scheme}://${location.host}${t.connectPath}`, [t.protocol, `ticket.${t.ticket}`]);
}

const minutes = (secs: number) => (secs % 60 === 0 ? `${secs / 60} minute${secs === 60 ? "" : "s"}` : `${secs} seconds`);

/** Plain words for why a session ended. */
export function endedBecause(reason: string, idleSecs: number, maxSecs: number): string {
  switch (reason) {
    case "exited": return "The shell exited.";
    case "idle": return `Ended after ${minutes(idleSecs)} without typing.`;
    case "max-duration": return `Ended at the time limit of ${minutes(maxSecs)}. Open a new session to continue.`;
    case "terminated": return "Ended by an administrator or from the sessions list.";
    case "access-revoked": return "Ended because your access to this application changed.";
    case "access-unverifiable": return "Ended because the control plane could not confirm your access.";
    case "rate-limited": return "Ended because input arrived too fast.";
    case "client-closed": return "You ended the session.";
    case "stalled": return "Ended because the connection stopped accepting data.";
    case "error": return "The session could not be kept open.";
    default: return "The session ended.";
  }
}

export { ApiRequestError };
