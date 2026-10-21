// SPDX-License-Identifier: AGPL-3.0-only
// The browser side of brokered sessions (terminal, PowerShell), shared by the Studio and the
// Fabric Console. The control plane enforces every rule (permission, approval, ticket, origin,
// limits); this file only speaks its protocol and puts its answers into words.

import type { SessionKind, SessionTicket } from "@apelora/api-client";

export type Kind = SessionKind;

export const KIND_LABEL: Record<Kind, string> = { terminal: "Terminal", powershell: "PowerShell", rdp: "Remote desktop" };
export const KIND_ACTION: Record<Kind, string> = { terminal: "Open terminal", powershell: "Open PowerShell", rdp: "Connect with RDP" };

/** The permission a kind of session needs; also the action of its approval request. */
export const permissionOf = (kind: Kind): "instance.console" | "instance.desktop" => (kind === "rdp" ? "instance.desktop" : "instance.console");

/** What an approval for a session on one instance is requested for (`instance:<application>/<instance>`). */
export const approvalTarget = (application: string, instance: string) => `instance:${application}/${instance}`;

/**
 * The address of the session's websocket: always this page's own origin (the API is reached
 * through the same host), never with the ticket in it.
 */
export function socketUrl(t: Pick<SessionTicket, "connectPath">, where: Pick<Location, "protocol" | "host"> = location): string {
  const scheme = where.protocol === "https:" ? "wss" : "ws";
  return `${scheme}://${where.host}${t.connectPath}`;
}

/** The sub-protocols the websocket offers: the session protocol, and the ticket riding along. */
export const socketProtocols = (t: Pick<SessionTicket, "protocol" | "ticket">): string[] => [t.protocol, `ticket.${t.ticket}`];

/** Opens the session's websocket. The ticket travels as a sub-protocol, never in the URL. */
export function openSocket(t: SessionTicket): WebSocket {
  return new WebSocket(socketUrl(t), socketProtocols(t));
}

const minutes = (secs: number) => (secs % 60 === 0 ? `${secs / 60} minute${secs === 60 ? "" : "s"}` : `${secs} seconds`);

/** Plain words for why a session ended (the reasons the control plane sends when it closes one). */
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

/** A short "12 min" / "45 s" for a duration in seconds (never negative). */
export function shortDuration(secs: number): string {
  const s = Math.max(0, Math.round(secs));
  if (s < 60) return `${s} s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m} min`;
  const h = Math.floor(m / 60);
  return m % 60 ? `${h} h ${m % 60} min` : `${h} h`;
}

/**
 * How long a running session has left, and which limit ends it: the idle limit (unless the
 * person types) or the absolute one. Times are RFC 3339 from the sessions list.
 */
export function remaining(s: { endsBy: string; idleEndsAt: string }, now: number): { secs: number; by: "idle" | "limit" } {
  const limit = Date.parse(s.endsBy);
  const idle = Date.parse(s.idleEndsAt);
  const by = idle < limit ? "idle" : "limit";
  return { secs: Math.max(0, ((by === "idle" ? idle : limit) - now) / 1000), by };
}
