import { completeOidcSignIn } from "@apelora/api-client";
import { create } from "zustand";

import { client, messageOf, readToken, writeToken } from "../deploy/api";
import { navigate } from "../router";
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

// Back from the identity provider (?code&state): the code can be exchanged once, and effects may
// run twice (StrictMode), so the page load shares one exchange.
let oidcReturn: ReturnType<typeof completeOidcSignIn> | null = null;
const finishOidcSignIn = () => (oidcReturn ??= completeOidcSignIn(new URL(window.location.href)));
// The token the provider issued on this page load, so a refusal of it is explained as such.
let providerToken: string | null = null;

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
    if (new URL(window.location.href).searchParams.has("state")) {
      try {
        const r = await finishOidcSignIn();
        if (r) { writeToken(r.token); providerToken = r.token; }
        navigate(r?.returnTo || window.location.pathname, { replace: true });
        if (window.location.search) window.history.replaceState(null, "", window.location.pathname + window.location.hash);
      } catch (e) {
        window.history.replaceState(null, "", window.location.pathname);
        set({ status: "ready", backend: local, who: null, reason: messageOf(e) });
        return;
      }
    }
    if (!readToken()) {
      let reachable = true;
      try { await client.get("health"); } catch { reachable = false; }
      set({ status: "ready", backend: local, who: null, reason: reachable ? "Not signed in to the control plane." : "No control plane is reachable." });
      return;
    }
    const token = readToken();
    try {
      const me = await client.get("whoami");
      set({ status: "ready", backend: api, who: me.name, reason: null });
    } catch (e) {
      const unauth = e instanceof Error && "status" in e && (e as { status: number }).status === 401;
      if (unauth) writeToken(null);
      set({
        status: "ready", backend: local, who: null,
        reason: unauth
          ? token !== null && token === providerToken ? `You signed in, but Apelora did not accept the identity provider's token: ${messageOf(e)}` : "The saved sign-in was not accepted."
          : "The control plane could not be reached.",
      });
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
    // Ends an Apelora session on the server (the request takes the token before it is forgotten
    // here); other credentials are untouched there. The tab forgets the token either way.
    void client.request("sign_out", {}).catch(() => undefined);
    writeToken(null);
    set({ backend: local, who: null, reason: "Signed out of the control plane.", status: "ready" });
  },
}));
