import { expect, test } from "vitest";

import { ago, bytes } from "./Health";

test("ago never goes negative and names the largest units", () => {
  expect(ago(-5)).toBe("0 s");
  expect(ago(0)).toBe("0 s");
  expect(ago(45)).toBe("45 s");
  expect(ago(59)).toBe("59 s");
  expect(ago(60)).toBe("1 min");
  expect(ago(3599)).toBe("59 min");
  expect(ago(3600)).toBe("1 h");
  expect(ago(3660)).toBe("1 h 1 min");
  expect(ago(86_400)).toBe("1 d");
  expect(ago(90_000)).toBe("1 d 1 h");
});

test("bytes switches to GiB at a gibibyte", () => {
  expect(bytes(1024 ** 2)).toBe("1 MiB");
  expect(bytes(1024 ** 3)).toBe("1.0 GiB");
  expect(bytes(100 * 1024 ** 3)).toBe("100 GiB");
});
