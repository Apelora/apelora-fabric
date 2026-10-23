import { afterEach, expect, test, vi } from "vitest";

import { installWindowLogging, log } from "./log";

afterEach(() => {
  vi.restoreAllMocks();
});

test("writes a JSON line and redacts secret field names", () => {
  const error = vi.spyOn(console, "error").mockImplementation(() => undefined);
  log("error", "sign-in failed", { token: "apl_secret", code: "unauthorized" });
  expect(error).toHaveBeenCalledTimes(1);
  const line = String(error.mock.calls[0]?.[0]);
  const event = JSON.parse(line) as { level: string; msg: string; token: string; code: string };
  expect(event.level).toBe("error");
  expect(event.msg).toBe("sign-in failed");
  expect(event.token).toBe("[redacted]");
  expect(event.code).toBe("unauthorized");
  expect(line).not.toContain("apl_secret");
});

test("routes levels to the matching console method", () => {
  const error = vi.spyOn(console, "error").mockImplementation(() => undefined);
  const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
  const info = vi.spyOn(console, "info").mockImplementation(() => undefined);
  log("error", "e");
  log("warn", "w");
  log("info", "i");
  log("debug", "d");
  expect(error).toHaveBeenCalledTimes(1);
  expect(warn).toHaveBeenCalledTimes(1);
  expect(info).toHaveBeenCalledTimes(2);
  expect(JSON.parse(String(warn.mock.calls[0]?.[0])).level).toBe("warn");
  expect(JSON.parse(String(info.mock.calls[1]?.[0])).level).toBe("debug");
});

test("redacts every secret-shaped field name and keeps the rest", () => {
  const info = vi.spyOn(console, "info").mockImplementation(() => undefined);
  log("info", "fields", {
    password: "p",
    secret: "s",
    authorization: "Bearer x",
    cookie: "sid=1",
    apikey: "k",
    api_key: "k2",
    pass: "hidden",
    path: "/api/v1/health",
  });
  const event = JSON.parse(String(info.mock.calls[0]?.[0])) as Record<string, string>;
  expect(event.password).toBe("[redacted]");
  expect(event.secret).toBe("[redacted]");
  expect(event.authorization).toBe("[redacted]");
  expect(event.cookie).toBe("[redacted]");
  expect(event.apikey).toBe("[redacted]");
  expect(event.api_key).toBe("[redacted]");
  expect(event.pass).toBe("[redacted]");
  expect(event.path).toBe("/api/v1/health");
  expect(JSON.stringify(event)).not.toContain("Bearer");
});

test("installWindowLogging records uncaught errors and unhandled rejections", () => {
  const error = vi.spyOn(console, "error").mockImplementation(() => undefined);
  const listeners = new Map<string, (event: unknown) => void>();
  vi.stubGlobal("window", {
    addEventListener: (type: string, fn: (event: unknown) => void) => {
      listeners.set(type, fn);
    },
  });
  installWindowLogging();
  listeners.get("error")?.({ message: "boom" });
  listeners.get("unhandledrejection")?.({ reason: new Error("nope") });
  expect(error).toHaveBeenCalledTimes(2);
  expect(JSON.parse(String(error.mock.calls[0]?.[0])).msg).toBe("uncaught error");
  expect(JSON.parse(String(error.mock.calls[1]?.[0])).msg).toBe("unhandled rejection");
  expect(JSON.parse(String(error.mock.calls[0]?.[0])).message).toBe("boom");
  expect(JSON.parse(String(error.mock.calls[1]?.[0])).reason).toContain("nope");
});
