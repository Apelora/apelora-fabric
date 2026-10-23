import { expect, test } from "vitest";

import { every } from "./Storage";

test("every names weeks, days, hours and leftover minutes", () => {
  expect(every(10080)).toBe("every week");
  expect(every(20160)).toBe("every 2 weeks");
  expect(every(1440)).toBe("every day");
  expect(every(2880)).toBe("every 2 days");
  expect(every(60)).toBe("every hour");
  expect(every(180)).toBe("every 3 hours");
  expect(every(15)).toBe("every 15 minutes");
  expect(every(1)).toBe("every 1 minutes");
});
