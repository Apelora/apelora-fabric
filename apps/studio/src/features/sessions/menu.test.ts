// SPDX-License-Identifier: AGPL-3.0-only
import type { AccessList } from "@apelora/api-client";
import { beforeEach, describe, expect, it } from "vitest";

import { sessionMenuItems } from "./menu";
import { useSessions } from "./sessions";

const windows: AccessList = {
  observed: true,
  provider: "mock",
  idleSecs: 900,
  maxSecs: 3600,
  items: [{
    instance: "shop-win-01", module: "win-01", guestOs: "windows", status: "Running",
    kinds: [
      { kind: "powershell", allowed: true, approvalRequired: false },
      { kind: "rdp", allowed: true, approvalRequired: false },
    ],
  }],
} as AccessList;

describe("session menu", () => {
  beforeEach(() => useSessions.setState({ app: "shop", access: windows, target: null }));

  it("offers PowerShell here and sends a permitted remote desktop to the Fabric Console", () => {
    const [ps, rdp] = sessionMenuItems("win-01");
    expect(ps).toMatchObject({ label: "Open PowerShell… (shop-win-01)", disabled: false });
    expect(rdp).toMatchObject({ disabled: true, hint: "Open it from the Fabric Console: it uses the desktop tunnel on your computer" });
  });

  it("keeps the control plane's reason when a remote desktop is not available at all", () => {
    useSessions.setState({
      access: { ...windows, items: [{ ...windows.items[0]!, kinds: [{ kind: "rdp", allowed: false, reason: "the remote desktop gateway is not set up on this control plane", approvalRequired: false }] }] },
    });
    expect(sessionMenuItems("win-01")[0]).toMatchObject({ disabled: true, hint: "the remote desktop gateway is not set up on this control plane" });
  });
});
