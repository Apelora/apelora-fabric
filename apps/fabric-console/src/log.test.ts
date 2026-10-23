import { afterEach, expect, test, vi } from "vitest";

import { log } from "./log";

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
