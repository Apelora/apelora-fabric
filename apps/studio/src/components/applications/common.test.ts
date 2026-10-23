import { expect, test } from "vitest";

import { timeAgo } from "./common";

const NOW = Date.parse("2026-10-23T12:00:00Z");

test("timeAgo uses just now, minutes, hours, days, then a date", () => {
  expect(timeAgo(null, NOW)).toBe("—");
  expect(timeAgo(undefined, NOW)).toBe("—");
  expect(timeAgo("not-a-date", NOW)).toBe("—");
  expect(timeAgo("2026-10-23T11:59:30Z", NOW)).toBe("just now");
  expect(timeAgo("2026-10-23T11:10:00Z", NOW)).toMatch(/minute/);
  expect(timeAgo("2026-10-23T09:00:00Z", NOW)).toMatch(/hour/);
  expect(timeAgo("2026-10-21T12:00:00Z", NOW)).toMatch(/day/);
  expect(timeAgo("2026-01-01T00:00:00Z", NOW)).not.toBe("—");
  expect(timeAgo("2026-01-01T00:00:00Z", NOW)).not.toBe("just now");
});
