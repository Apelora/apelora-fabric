import { expect, test } from "vitest";

import { duration } from "./Alerts";

test("duration names seconds, minutes, hours and days", () => {
  expect(duration(0)).toBe("at once");
  expect(duration(-3)).toBe("at once");
  expect(duration(45)).toBe("45 s");
  expect(duration(59)).toBe("59 s");
  expect(duration(60)).toBe("1 min");
  expect(duration(90)).toBe("2 min");
  expect(duration(3600)).toBe("1 h");
  expect(duration(5400)).toBe("1.5 h");
  expect(duration(86_400)).toBe("1 day");
  expect(duration(172_800)).toBe("2 days");
});
