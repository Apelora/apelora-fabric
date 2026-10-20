// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import { codeChallenge, completeOidcSignIn, discover, startOidcSignIn } from "./oidc";

class MemoryStorage implements Storage {
  private m = new Map<string, string>();
  get length() { return this.m.size; }
  clear() { this.m.clear(); }
  getItem(k: string) { return this.m.get(k) ?? null; }
  key(i: number) { return [...this.m.keys()][i] ?? null; }
  removeItem(k: string) { this.m.delete(k); }
  setItem(k: string, v: string) { this.m.set(k, v); }
}

const issuer = "https://id.example.test/realms/apelora";
const settings = { issuer, clientId: "apelora-console", scopes: ["openid", "profile"], label: "Example" };
const discovery = { issuer, authorization_endpoint: `${issuer}/protocol/openid-connect/auth`, token_endpoint: `${issuer}/protocol/openid-connect/token` };
const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
const idToken = (nonce: string) => `h.${btoa(JSON.stringify({ nonce })).replace(/=+$/, "")}.s`;

describe("OIDC sign-in in the browser", () => {
  it("sends the browser to the provider with PKCE, then exchanges the code once the state matches", async () => {
    const storage = new MemoryStorage();
    let went = "";
    await startOidcSignIn(settings, "https://console.example.test/", "#/instances", { storage, fetchImpl: async () => json(discovery), navigate: (u) => { went = u; } });
    const auth = new URL(went);
    expect(auth.origin + auth.pathname).toBe(discovery.authorization_endpoint);
    const p = auth.searchParams;
    expect([p.get("response_type"), p.get("client_id"), p.get("scope"), p.get("code_challenge_method")]).toEqual(["code", "apelora-console", "openid profile", "S256"]);
    const pending = JSON.parse(storage.getItem("apelora.oidc.pending")!) as { verifier: string; nonce: string; state: string };
    expect(p.get("code_challenge")).toBe(await codeChallenge(pending.verifier));
    expect(pending.verifier).toMatch(/^[A-Za-z0-9_-]{43}$/);

    let sent: URLSearchParams | null = null;
    const back = new URL(`https://console.example.test/?code=abc&state=${p.get("state")}`);
    const r = await completeOidcSignIn(back, { storage, fetchImpl: async (_u, init) => { sent = new URLSearchParams(String(init?.body)); return json({ access_token: "at-1", id_token: idToken(pending.nonce) }); } });
    expect(r).toEqual({ token: "at-1", returnTo: "#/instances" });
    expect(sent!.get("code_verifier")).toBe(pending.verifier);
    expect(sent!.get("grant_type")).toBe("authorization_code");
    expect(storage.getItem("apelora.oidc.pending")).toBeNull();
  });

  it("refuses a state it did not start, a nonce that does not match, and says what the provider said", async () => {
    const storage = new MemoryStorage();
    await startOidcSignIn(settings, "https://console.example.test/", "", { storage, fetchImpl: async () => json(discovery), navigate: () => {} });
    const state = (JSON.parse(storage.getItem("apelora.oidc.pending")!) as { state: string }).state;
    const raw = storage.getItem("apelora.oidc.pending")!;
    await expect(completeOidcSignIn(new URL("https://c.test/?code=x&state=forged"), { storage })).rejects.toThrow("not started in this tab");
    storage.setItem("apelora.oidc.pending", raw);
    await expect(completeOidcSignIn(new URL(`https://c.test/?code=x&state=${state}`), { storage, fetchImpl: async () => json({ access_token: "t", id_token: idToken("other") }) })).rejects.toThrow("did not match");
    await expect(completeOidcSignIn(new URL("https://c.test/?error=access_denied&error_description=User%20cancelled"), { storage })).rejects.toThrow("User cancelled");
    expect(await completeOidcSignIn(new URL("https://c.test/#/overview"), { storage })).toBeNull();
  });

  it("only talks to a secure provider whose discovery document names the expected issuer", async () => {
    await expect(discover("http://id.example.test")).rejects.toThrow("not secure");
    await expect(discover(issuer, async () => json({ ...discovery, issuer: "https://evil.example.test" }))).rejects.toThrow("different issuer");
    await expect(discover(issuer, async () => json({ ...discovery, token_endpoint: "http://id.example.test/token" }))).rejects.toThrow("not secure");
    expect(await discover("http://127.0.0.1:5556", async () => json({ issuer: "http://127.0.0.1:5556", authorization_endpoint: "http://127.0.0.1:5556/auth", token_endpoint: "http://127.0.0.1:5556/token" }))).toEqual({ authorization: "http://127.0.0.1:5556/auth", token: "http://127.0.0.1:5556/token" });
  });
});
