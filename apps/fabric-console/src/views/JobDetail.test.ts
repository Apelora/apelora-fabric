import { expect, test } from "vitest";

import { duration } from "./JobDetail";

test("duration is an em dash without both times, then seconds, minutes and hours", () => {
  expect(duration()).toBe("—");
  expect(duration("2026-01-01T00:00:00Z")).toBe("—");
  expect(duration("2026-01-01T00:00:00Z", "2026-01-01T00:00:04Z")).toBe("4 s");
  expect(duration("2026-01-01T00:00:00Z", "2026-01-01T00:02:05Z")).toBe("2 min 5 s");
  expect(duration("2026-01-01T00:00:00Z", "2026-01-01T01:05:00Z")).toBe("1 h 5 min");
  expect(duration("2026-01-01T00:00:10Z", "2026-01-01T00:00:00Z")).toBe("0 s");
});
