import { expect, test } from "vitest";

import { TAG_NAME } from "./tags";

test("tag names are lowercase, start with a letter or digit, and stay at 32 characters", () => {
  expect(TAG_NAME.test("a")).toBe(true);
  expect(TAG_NAME.test("web.http")).toBe(true);
  expect(TAG_NAME.test("db_1")).toBe(true);
  expect(TAG_NAME.test("a".repeat(32))).toBe(true);
  expect(TAG_NAME.test("")).toBe(false);
  expect(TAG_NAME.test("A")).toBe(false);
  expect(TAG_NAME.test("-web")).toBe(false);
  expect(TAG_NAME.test("a".repeat(33))).toBe(false);
});
