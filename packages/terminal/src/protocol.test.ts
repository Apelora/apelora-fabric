// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";

import { approvalTarget, endedBecause, permissionOf, remaining, shortDuration, socketProtocols, socketUrl } from "./protocol";

describe("session protocol", () => {
  const ticket = { connectPath: "/api/v1/sessions/connect", protocol: "apelora.session.v1", ticket: "abc123" };

  it("connects to the page's own origin and never puts the ticket in the address", () => {
    expect(socketUrl(ticket, { protocol: "https:", host: "console.example.internal" })).toBe("wss://console.example.internal/api/v1/sessions/connect");
    expect(socketUrl(ticket, { protocol: "http:", host: "127.0.0.1:5174" })).toBe("ws://127.0.0.1:5174/api/v1/sessions/connect");
    expect(socketUrl(ticket, { protocol: "http:", host: "x" })).not.toContain("abc123");
    expect(socketProtocols(ticket)).toEqual(["apelora.session.v1", "ticket.abc123"]);
  });

  it("names the approval the control plane expects", () => {
    expect(approvalTarget("shop", "shop-web-01")).toBe("instance:shop/shop-web-01");
    expect(permissionOf("terminal")).toBe("instance.console");
    expect(permissionOf("powershell")).toBe("instance.console");
    expect(permissionOf("rdp")).toBe("instance.desktop");
  });

  it("says why a session ended in plain words", () => {
    expect(endedBecause("idle", 900, 3600)).toBe("Ended after 15 minutes without typing.");
    expect(endedBecause("max-duration", 900, 3600)).toContain("60 minutes");
    expect(endedBecause("terminated", 900, 3600)).toContain("sessions list");
    expect(endedBecause("idle", 45, 3600)).toBe("Ended after 45 seconds without typing.");
    expect(endedBecause("something-new", 900, 3600)).toBe("The session ended.");
  });

  it("counts down to the limit that comes first", () => {
    const now = Date.parse("2026-10-21T10:00:00Z");
    expect(remaining({ endsBy: "2026-10-21T10:50:00Z", idleEndsAt: "2026-10-21T10:12:00Z" }, now)).toEqual({ secs: 720, by: "idle" });
    expect(remaining({ endsBy: "2026-10-21T10:05:00Z", idleEndsAt: "2026-10-21T10:12:00Z" }, now)).toEqual({ secs: 300, by: "limit" });
    expect(remaining({ endsBy: "2026-10-21T09:00:00Z", idleEndsAt: "2026-10-21T09:00:00Z" }, now).secs).toBe(0);
    expect(shortDuration(45)).toBe("45 s");
    expect(shortDuration(720)).toBe("12 min");
    expect(shortDuration(3600)).toBe("1 h");
    expect(shortDuration(3900)).toBe("1 h 5 min");
    expect(shortDuration(-3)).toBe("0 s");
  });
});
