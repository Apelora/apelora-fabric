import { describe, expect, it, vi } from "vitest";
import { ApiRequestError, createClient, followJob, operations } from "./index";

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

  it("sends an approval and an idempotency key only when given", async () => {
    const fetch = vi.fn(async () => json(201, {}));
    const api = createClient({ token: () => "t", fetch });
    const opts = { params: { name: "shop", instance: "shop-web-01" }, body: { kind: "terminal" as const } };
    await api.request("open_session", opts);
    await api.request("open_session", { ...opts, approvalId: "a-1", idempotencyKey: "k-1" });
    const h = fetch.mock.calls.map((c) => (c as unknown as [string, RequestInit])[1].headers as Record<string, string>);
    expect(h[0]?.["apelora-approval"]).toBeUndefined();
    expect(h[0]?.["idempotency-key"]).toBeUndefined();
    expect(h[1]?.["apelora-approval"]).toBe("a-1");
    expect(h[1]?.["idempotency-key"]).toBe("k-1");
  });

  it("returns the metrics exposition as text, with the token", async () => {
    const fetch = vi.fn(async () => new Response("# HELP apelora_build_info x\n", { status: 200, headers: { "content-type": "text/plain; version=0.0.4; charset=utf-8" } }));
    const api = createClient({ token: () => "aps_x", fetch });
    await expect(api.get("metrics")).resolves.toBe("# HELP apelora_build_info x\n");
    const [url, init] = fetch.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe("/metrics");
    expect((init.headers as Record<string, string>).authorization).toBe("Bearer aps_x");
  });

  it("returns audit exports as text, CSV and JSON lines alike, with the filters in the query", async () => {
    const answers = [
      new Response("id,at\r\n1,2026\r\n", { status: 200, headers: { "content-type": "text/csv; charset=utf-8" } }),
      new Response('{"id":1}\n', { status: 200, headers: { "content-type": "application/x-ndjson" } }),
    ];
    const fetch = vi.fn(async () => answers.shift() as Response);
    const api = createClient({ token: () => "apu_x", fetch });
    await expect(api.request("audit_export", { query: { format: "csv", actor: "ada", outcome: "denied" } })).resolves.toBe("id,at\r\n1,2026\r\n");
    await expect(api.request("audit_export", { query: { format: "jsonl" } })).resolves.toBe('{"id":1}\n');
    const [url] = fetch.mock.calls[0] as unknown as [string];
    expect(url).toBe("/api/v1/audit/export?format=csv&actor=ada&outcome=denied");
  });

  it("returns files to save (the support bundle) as a Blob, with the token", async () => {
    const fetch = vi.fn(async () => new Response(new Uint8Array([117, 115, 116, 97, 114]), { status: 200, headers: { "content-type": "application/x-tar" } }));
    const api = createClient({ token: () => "apu_x", fetch });
    const blob = await api.get("support_bundle");
    expect(blob).toBeInstanceOf(Blob);
    expect(await blob.text()).toBe("ustar");
    const [url, init] = fetch.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe("/api/v1/support-bundle");
    expect((init.headers as Record<string, string>).authorization).toBe("Bearer apu_x");
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

  it("fills path parameters, sends bodies and idempotency keys", async () => {
    const fetch = vi.fn(async () => json(202, { jobId: "j1", created: true }));
    const api = createClient({ token: () => "apl_x", fetch });
    await api.request("change_state", { params: { project: "apelora", name: "web 01" }, body: { action: "stop" }, idempotencyKey: "k1" });
    await api.request("delete_instance", { params: { project: "apelora", name: "web-01" } });
    const [url, init] = fetch.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe("/api/v1/instances/apelora/web%2001/state");
    expect(init.method).toBe("POST");
    expect(init.body).toBe(JSON.stringify({ action: "stop" }));
    expect(init.headers).toMatchObject({ "idempotency-key": "k1", "content-type": "application/json", authorization: "Bearer apl_x" });
    const [delUrl, delInit] = fetch.mock.calls[1] as unknown as [string, RequestInit];
    expect([delUrl, delInit.method, delInit.body]).toEqual(["/api/v1/instances/apelora/web-01", "DELETE", undefined]);
  });

  it("adds only the defined query parameters, encoded", async () => {
    const fetch = vi.fn(async () => json(200, { items: [] }));
    const api = createClient({ token: () => "apl_x", fetch });
    await api.request("lookup_user", { query: { q: "a b@example.org" } });
    await api.request("identity_audit", { query: { action: "role.", outcome: undefined, before: 41, limit: 50 } });
    await api.request("assignments", { query: { includeRevoked: false } });
    await api.get("users");
    expect(fetch.mock.calls.map((c) => (c as unknown as [string])[0])).toEqual([
      "/api/v1/identity/users/lookup?q=a+b%40example.org",
      "/api/v1/identity/audit?action=role.&before=41&limit=50",
      "/api/v1/identity/assignments?includeRevoked=false",
      "/api/v1/identity/users",
    ]);
  });

  it("returns null for 204 No Content and still reports errors", async () => {
    const fetch = vi.fn(async () => new Response(null, { status: 204 }));
    const api = createClient({ token: () => "apl_x", fetch });
    await expect(api.request("revoke_join_token", { params: { member: "m2" } })).resolves.toBeNull();
    expect((fetch.mock.calls[0] as unknown as [string, RequestInit])[1].method).toBe("DELETE");
    const missing = createClient({ fetch: async () => json(404, { error: { code: "not_found", message: "no pending join token for m2" } }) });
    await expect(missing.request("revoke_join_token", { params: { member: "m2" } })).rejects.toMatchObject({ status: 404, code: "not_found" });
  });

  it("follows a job until it is final", async () => {
    const states = ["pending", "running", "succeeded"];
    const fetch = vi.fn(async () => json(200, { id: "j1", kind: "instance.create", status: states.shift(), attempts: 1, maxAttempts: 5, lastError: null, incusOperation: null, request: {}, createdAt: "", updatedAt: "" }));
    const seen: string[] = [];
    const job = await followJob(createClient({ fetch }), "j1", (j) => seen.push(j.status), { intervalMs: 1 });
    expect(job.status).toBe("succeeded");
    expect(seen).toEqual(["pending", "running", "succeeded"]);
    expect((fetch.mock.calls[0] as unknown as [string])[0]).toBe("/api/v1/jobs/j1");
  });

  it("rejects incomplete requests at compile time", () => {
    const api = createClient({ fetch: async () => json(200, {}) });
    const never = () => {
      // @ts-expect-error a body is required
      void api.request("change_state", { params: { project: "a", name: "b" } });
      // @ts-expect-error path parameters are required
      void api.request("delete_instance", {});
      // @ts-expect-error get() is only for operations without parameters or a body
      void api.get("job");
      // @ts-expect-error the action must be start or stop
      void api.request("change_state", { params: { project: "a", name: "b" }, body: { action: "reboot" } });
    };
    expect(typeof never).toBe("function");
  });

  it("covers every route of the control plane", () => {
    expect(Object.values(operations).map((o) => `${o.method} ${o.path}`).sort()).toEqual([
      "DELETE /api/v1/alerts/channels/{id}",
      "DELETE /api/v1/alerts/silences/{id}",
      "DELETE /api/v1/applications/{name}/volumes/{volume}",
      "DELETE /api/v1/applications/{name}/volumes/{volume}/snapshots/{snapshot}",
      "DELETE /api/v1/cluster/join-tokens/{member}",
      "DELETE /api/v1/identity/assignments/{assignment}",
      "DELETE /api/v1/identity/groups/{group}/members/{user}",
      "DELETE /api/v1/identity/personal-tokens/{id}",
      "DELETE /api/v1/identity/policies/{id}",
      "DELETE /api/v1/identity/roles/{role}",
      "DELETE /api/v1/identity/sign-in/directories/{id}",
      "DELETE /api/v1/identity/sign-in/providers/{id}",
      "DELETE /api/v1/identity/sign-ins/{id}",
      "DELETE /api/v1/instances/{project}/{name}",
      "DELETE /api/v1/machines/{name}",
      "DELETE /api/v1/machines/{name}/approval",
      "DELETE /api/v1/me/sign-ins/{id}",
      "DELETE /api/v1/me/tokens/{id}",
      "DELETE /api/v1/sessions/{id}",
      "DELETE /api/v1/tokens/{id}",
      "GET /api/v1/alerts",
      "GET /api/v1/alerts/channels",
      "GET /api/v1/alerts/history",
      "GET /api/v1/alerts/rules",
      "GET /api/v1/applications",
      "GET /api/v1/applications/{name}",
      "GET /api/v1/applications/{name}/access",
      "GET /api/v1/applications/{name}/draft",
      "GET /api/v1/applications/{name}/events",
      "GET /api/v1/applications/{name}/operations",
      "GET /api/v1/applications/{name}/operations/{id}",
      "GET /api/v1/applications/{name}/status",
      "GET /api/v1/applications/{name}/versions",
      "GET /api/v1/applications/{name}/versions/{revision}",
      "GET /api/v1/applications/{name}/volumes",
      "GET /api/v1/applications/{name}/volumes/{volume}/backups",
      "GET /api/v1/applications/{name}/volumes/{volume}/backups/{backup}/restore",
      "GET /api/v1/applications/{name}/volumes/{volume}/snapshots",
      "GET /api/v1/approvals",
      "GET /api/v1/audit",
      "GET /api/v1/audit/export",
      "GET /api/v1/capacity",
      "GET /api/v1/cluster",
      "GET /api/v1/cluster/groups",
      "GET /api/v1/cluster/join-tokens",
      "GET /api/v1/cluster/members",
      "GET /api/v1/cluster/members/{member}/evacuation",
      "GET /api/v1/cluster/members/{member}/removal",
      "GET /api/v1/cluster/settings",
      "GET /api/v1/health",
      "GET /api/v1/health/components",
      "GET /api/v1/health/servers",
      "GET /api/v1/identity/assignments",
      "GET /api/v1/identity/audit",
      "GET /api/v1/identity/groups",
      "GET /api/v1/identity/groups/{group}/members",
      "GET /api/v1/identity/organizations",
      "GET /api/v1/identity/permissions",
      "GET /api/v1/identity/personal-tokens",
      "GET /api/v1/identity/policies",
      "GET /api/v1/identity/projects",
      "GET /api/v1/identity/projects/{project}/environments",
      "GET /api/v1/identity/roles",
      "GET /api/v1/identity/service-identities",
      "GET /api/v1/identity/sign-in",
      "GET /api/v1/identity/sign-in/directories",
      "GET /api/v1/identity/sign-ins",
      "GET /api/v1/identity/users",
      "GET /api/v1/identity/users/lookup",
      "GET /api/v1/image-mirrors",
      "GET /api/v1/instances",
      "GET /api/v1/instances/{project}/{name}",
      "GET /api/v1/instances/{project}/{name}/console-log",
      "GET /api/v1/instances/{project}/{name}/logs",
      "GET /api/v1/instances/{project}/{name}/logs/{file}",
      "GET /api/v1/instances/{project}/{name}/move",
      "GET /api/v1/jobs",
      "GET /api/v1/jobs/{id}",
      "GET /api/v1/jobs/{id}/detail",
      "GET /api/v1/machines",
      "GET /api/v1/machines/{name}/admission",
      "GET /api/v1/me/sign-ins",
      "GET /api/v1/me/tokens",
      "GET /api/v1/monitoring",
      "GET /api/v1/networks",
      "GET /api/v1/networks/applications",
      "GET /api/v1/networks/settings",
      "GET /api/v1/preflight",
      "GET /api/v1/projects",
      "GET /api/v1/sessions",
      "GET /api/v1/settings",
      "GET /api/v1/sign-in",
      "GET /api/v1/storage-pools",
      "GET /api/v1/storage/orphans",
      "GET /api/v1/storage/pools",
      "GET /api/v1/storage/volumes",
      "GET /api/v1/support-bundle",
      "GET /api/v1/system",
      "GET /api/v1/tokens",
      "GET /api/v1/whoami",
      "GET /metrics",
      "PATCH /api/v1/applications/{name}",
      "PATCH /api/v1/applications/{name}/volumes/{volume}",
      "POST /api/v1/alerts/channels",
      "POST /api/v1/alerts/channels/{id}/test",
      "POST /api/v1/alerts/silences",
      "POST /api/v1/alerts/{id}/acknowledge",
      "POST /api/v1/applications",
      "POST /api/v1/applications/{name}/archive",
      "POST /api/v1/applications/{name}/deployments",
      "POST /api/v1/applications/{name}/instances/{instance}/sessions",
      "POST /api/v1/applications/{name}/operations",
      "POST /api/v1/applications/{name}/plan",
      "POST /api/v1/applications/{name}/restore",
      "POST /api/v1/applications/{name}/versions",
      "POST /api/v1/applications/{name}/volumes/{volume}/backups",
      "POST /api/v1/applications/{name}/volumes/{volume}/backups/{backup}/restore",
      "POST /api/v1/applications/{name}/volumes/{volume}/snapshots",
      "POST /api/v1/applications/{name}/volumes/{volume}/snapshots/{snapshot}/restore",
      "POST /api/v1/approvals",
      "POST /api/v1/approvals/{id}/approve",
      "POST /api/v1/approvals/{id}/cancel",
      "POST /api/v1/approvals/{id}/reject",
      "POST /api/v1/cluster/join-tokens",
      "POST /api/v1/cluster/members/{member}/evacuate",
      "POST /api/v1/cluster/members/{member}/remove",
      "POST /api/v1/cluster/members/{member}/restore",
      "POST /api/v1/identity/assignments",
      "POST /api/v1/identity/assignments/preview",
      "POST /api/v1/identity/emergency-access",
      "POST /api/v1/identity/groups",
      "POST /api/v1/identity/local-accounts",
      "POST /api/v1/identity/local-accounts/{id}/reset",
      "POST /api/v1/identity/organizations",
      "POST /api/v1/identity/organizations/{org}/projects",
      "POST /api/v1/identity/projects/{project}/environments",
      "POST /api/v1/identity/roles",
      "POST /api/v1/identity/service-identities",
      "POST /api/v1/identity/service-identities/{id}/disable",
      "POST /api/v1/identity/service-identities/{id}/enable",
      "POST /api/v1/identity/sign-in/check",
      "POST /api/v1/identity/sign-in/directories",
      "POST /api/v1/identity/sign-in/directories/check",
      "POST /api/v1/identity/sign-in/providers",
      "POST /api/v1/identity/users/{user}/disable",
      "POST /api/v1/identity/users/{user}/enable",
      "POST /api/v1/instances",
      "POST /api/v1/instances/{project}/{name}/move",
      "POST /api/v1/instances/{project}/{name}/state",
      "POST /api/v1/jobs/{id}/cancel",
      "POST /api/v1/jobs/{id}/retry",
      "POST /api/v1/machines",
      "POST /api/v1/machines/{name}/admit",
      "POST /api/v1/machines/{name}/approve",
      "POST /api/v1/me/tokens",
      "POST /api/v1/me/tokens/{id}/rotate",
      "POST /api/v1/sign-in/password",
      "POST /api/v1/sign-in/setup",
      "POST /api/v1/sign-out",
      "POST /api/v1/tokens",
      "PUT /api/v1/alerts/channels/{id}",
      "PUT /api/v1/alerts/rules/{rule}",
      "PUT /api/v1/applications/{name}/draft",
      "PUT /api/v1/applications/{name}/volumes/{volume}/schedules",
      "PUT /api/v1/identity/groups/{group}/members/{user}",
      "PUT /api/v1/identity/policies",
      "PUT /api/v1/identity/sign-in/api-tokens",
      "PUT /api/v1/identity/sign-in/directories/{id}",
      "PUT /api/v1/identity/sign-in/local-accounts",
      "PUT /api/v1/identity/sign-in/providers/{id}",
      "PUT /api/v1/me/password",
    ]);
  });
});
