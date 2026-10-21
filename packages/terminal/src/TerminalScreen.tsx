// SPDX-License-Identifier: AGPL-3.0-only
// The terminal of a brokered session: a terminal emulator wired to the session's websocket.
// Shared by the Studio and the Fabric Console; both pages get the same protocol handling.
// Output is written to the emulator as terminal text, never as HTML.

import { useEffect, useRef, type RefObject } from "react";
import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import type { SessionTicket } from "@apelora/api-client";

import { endedBecause, openSocket } from "./protocol";

const THEME = { background: "#1b1f24", foreground: "#e6e8eb", cursor: "#e6e8eb", selectionBackground: "#3b4452" };

export interface TerminalScreenProps {
  /** The single-use ticket from `open_session`. A new ticket opens a new connection. */
  ticket: SessionTicket;
  /** Accessible name of the terminal region, e.g. "Terminal of shop-web-01". */
  label: string;
  /** The websocket is open: the session is live. */
  onLive: () => void;
  /** The session ended; the reason in plain words. Called once. */
  onEnded: (message: string) => void;
  /** Something went wrong that the person should read (the session may still end later). */
  onError: (message: string) => void;
  /** Filled with a function that ends the session from outside (an "End session" button). */
  endRef?: RefObject<(() => void) | null>;
  testId?: string;
}

export function TerminalScreen({ ticket, label, onLive, onEnded, onError, endRef, testId = "session-terminal" }: TerminalScreenProps) {
  const host = useRef<HTMLDivElement>(null);
  // The callbacks may change on every render; the connection must not.
  const handlers = useRef({ onLive, onEnded, onError });
  handlers.current = { onLive, onEnded, onError };

  useEffect(() => {
    if (!host.current) return;
    const term = new Terminal({ cursorBlink: true, fontSize: 13, scrollback: 2000, theme: THEME, fontFamily: "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace" });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host.current);
    fit.fit();
    const enc = new TextEncoder();
    let ws: WebSocket | null = null;
    let finished = false;
    const finish = (why: string) => {
      if (finished) return;
      finished = true;
      handlers.current.onEnded(endedBecause(why, ticket.idleSecs, ticket.maxSecs));
      term.options.cursorBlink = false;
    };
    // The ticket works once. Connect on the next task, so a mount that is undone at once (React's
    // development double mount) never spends it on a socket that is closed straight away.
    const connect = setTimeout(() => {
      const socket = openSocket(ticket);
      ws = socket;
      socket.binaryType = "arraybuffer";
      socket.onopen = () => {
        handlers.current.onLive();
        socket.send(JSON.stringify({ type: "resize", cols: term.cols, rows: term.rows }));
        term.focus();
      };
      socket.onmessage = (ev: MessageEvent) => {
        if (typeof ev.data === "string") {
          try {
            const m = JSON.parse(ev.data) as { type?: string; reason?: string; message?: string };
            if (m.type === "closed") finish(m.reason ?? "");
            else if (m.type === "error") handlers.current.onError(m.message ?? "The session could not be opened.");
          } catch { /* not ours */ }
          return;
        }
        term.write(new Uint8Array(ev.data as ArrayBuffer));
      };
      socket.onclose = () => finish("client-closed");
      socket.onerror = () => handlers.current.onError("The connection to the control plane failed.");
    }, 0);
    const typed = term.onData((d) => { if (ws?.readyState === WebSocket.OPEN) ws.send(enc.encode(d)); });
    const sized = term.onResize(({ cols, rows }) => { if (ws?.readyState === WebSocket.OPEN) ws.send(JSON.stringify({ type: "resize", cols, rows })); });
    // Fit on the next frame: fitting inside the observer's callback resizes what it observes.
    let frame = 0;
    const watch = new ResizeObserver(() => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => { try { fit.fit(); } catch { /* hidden */ } });
    });
    watch.observe(host.current);
    if (endRef) endRef.current = () => ws?.close();
    return () => {
      finished = true;
      clearTimeout(connect);
      cancelAnimationFrame(frame);
      if (endRef) endRef.current = null;
      watch.disconnect();
      typed.dispose();
      sized.dispose();
      ws?.close();
      term.dispose();
    };
  }, [ticket, endRef]);

  return <div ref={host} className="min-h-72 flex-1 overflow-hidden rounded-box bg-[#1b1f24] p-2" role="region" aria-label={label} data-testid={testId} />;
}
