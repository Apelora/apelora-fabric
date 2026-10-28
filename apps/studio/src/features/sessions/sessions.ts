import { ApiRequestError, type AccessList, type InstanceAccess, type KindAccess, type SessionTicket } from "@apelora/api-client";
import { KIND_ACTION, KIND_LABEL, approvalTarget, endedBecause, openSocket, permissionOf } from "@apelora/terminal";
import { create } from "zustand";

import { client, messageOf } from "../deploy/api";

export type Kind = KindAccess["kind"];

// The protocol (labels, socket, reasons) is shared with the Fabric Console.
export { KIND_ACTION, KIND_LABEL, endedBecause, openSocket };

/** What the person chose to open, and what the dialog needs to know about it. */
export interface Target {
  app: string;
  instance: string;
  module: string;
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

/** The instances of one module (its replicas) that offer sessions, in name order. */
export const instancesOf = (access: AccessList | null, module: string): InstanceAccess[] =>
  (access?.items ?? []).filter((i) => i.module === module);

export const kindOf = (i: InstanceAccess, kind: Kind): KindAccess | undefined => i.kinds.find((k) => k.kind === kind);

export interface Opened {
  ticket: SessionTicket;
}

/** Asks the control plane for a single-use ticket. `approvalId` is the approval production needs. */
export async function openSession(t: Target, size: { cols: number; rows: number }, approvalId?: string): Promise<SessionTicket> {
  return client.request("open_remote_session", { params: { name: t.app, instance: t.instance }, body: { kind: t.kind, cols: size.cols, rows: size.rows }, approvalId: approvalId || undefined });
}

/** Asks another person to approve this instance's session. Returns the approval id. */
export async function requestApproval(t: Target, reason: string): Promise<string> {
  const a = await client.request("request_approval", { body: { action: permissionOf(t.kind), target: approvalTarget(t.app, t.instance), reason } });
  return a.id;
}

export { ApiRequestError };
