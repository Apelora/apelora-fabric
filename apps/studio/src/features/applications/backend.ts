import { create } from "zustand";

import { client, readToken, writeToken } from "../deploy/api";
import { createApiBackend } from "./apiBackend";
import { createLocalBackend } from "./localBackend";
import type { AppBackend } from "./model";

interface BackendState {
  status: "detecting" | "ready";
  backend: AppBackend;
  /** Why this browser's own storage is used instead of a control plane (shown to the person). */
  reason: string | null;
  who: string | null;
  detect: () => Promise<void>;
  /** Signs in with an API or OIDC access token (kept only for this browser tab) and switches to the control plane. */
  connect: (token: string) => Promise<void>;
  disconnect: () => void;
}

const local = createLocalBackend();
const api = createApiBackend();

/**
 * Which backend holds the applications. The control plane is used when a token is present and
 * the control plane answers; otherwise applications live in this browser and say so.
 */
export const useBackend = create<BackendState>((set, get) => ({
  status: "detecting",
  backend: local,
  reason: null,
  who: null,
  detect: async () => {
    if (!readToken()) {
      let reachable = true;
      try { await client.get("health"); } catch { reachable = false; }
      set({ status: "ready", backend: local, who: null, reason: reachable ? "Not signed in to the control plane." : "No control plane is reachable." });
      return;
    }
    try {
      const me = await client.get("whoami");
      set({ status: "ready", backend: api, who: me.name, reason: null });
    } catch (e) {
      const unauth = e instanceof Error && "status" in e && (e as { status: number }).status === 401;
      if (unauth) writeToken(null);
      set({ status: "ready", backend: local, who: null, reason: unauth ? "The saved sign-in was not accepted." : "The control plane could not be reached." });
    }
  },
  connect: async (token) => {
    writeToken(token.trim());
    await get().detect();
    if (get().backend.kind !== "api") {
      writeToken(null);
      throw new Error(get().reason ?? "Could not sign in.");
    }
  },
  disconnect: () => {
    writeToken(null);
    set({ backend: local, who: null, reason: "Signed out of the control plane.", status: "ready" });
  },
}));
