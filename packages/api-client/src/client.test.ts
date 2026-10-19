import { describe, expect, it, vi } from "vitest";
import { ApiRequestError, createClient, operations } from "./index";

const json = (status: number, body: unknown) => new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

describe("api client", () => {
  it("sends the token only to protected routes", async () => {
    const fetch = vi.fn(async () => json(200, { items: [] }));
    const api = createClient({ token: () => "apl_x", fetch });
    await api.get("instances");
    await api.get("health");
    const headers = fetch.mock.calls.map((c) => (c as unknown as [string, RequestInit])[1].headers as Record<string, string>);
    expect(fetch.mock.calls.map((c) => (c as unknown as [string])[0])).toEqual(["/api/v1/instances", "/api/v1/health"]);
    expect(headers[0]?.authorization).toBe("Bearer apl_x");
    expect(headers[1]?.authorization).toBeUndefined();
  });

  it("turns error bodies into typed errors with the stable code", async () => {
    const api = createClient({ fetch: async () => json(401, { error: { code: "unauthenticated", message: "token required" } }) });
    const err = await api.get("cluster").catch((e: unknown) => e);
    expect(err).toBeInstanceOf(ApiRequestError);
    expect(err).toMatchObject({ status: 401, code: "unauthenticated", message: "token required", unauthenticated: true });
  });

  it("reports unreachable servers and non-JSON bodies without inventing data", async () => {
    const down = createClient({ fetch: async () => { throw new TypeError("Failed to fetch"); } });
    await expect(down.get("health")).rejects.toMatchObject({ status: 0, code: "network_error" });
    const html = createClient({ fetch: async () => new Response("<html>", { status: 502 }) });
    await expect(html.get("health")).rejects.toMatchObject({ status: 502, code: "http_error" });
  });

  it("covers every route of the control plane", () => {
    expect(Object.values(operations).map((o) => o.path).sort()).toEqual(
      ["audit", "cluster", "health", "instances", "networks", "projects", "storage-pools", "whoami"].map((p) => `/api/v1/${p}`),
    );
  });
});
