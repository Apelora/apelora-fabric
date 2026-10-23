import { expect, test } from "vitest";

import { createEmptyAssembly, slug, trimHyphens, uniqueId } from "./ops";

test("trimHyphens strips only the ends and survives a string of hyphens", () => {
  expect(trimHyphens("--a-b--")).toBe("a-b");
  expect(trimHyphens("---")).toBe("");
  expect(trimHyphens("abc")).toBe("abc");
  expect(trimHyphens("")).toBe("");
});

test("slug is lowercase, hyphenated, 40 characters, and starts with a letter", () => {
  expect(slug("Hello World")).toBe("hello-world");
  expect(slug("NGINX")).toBe("nginx");
  expect(slug("123abc")).toBe("x-123abc");
  expect(slug("---")).toBe("x-");
  expect(slug("a".repeat(80)).length).toBe(40);
  expect(slug("a".repeat(80)).startsWith("a")).toBe(true);
});

test("uniqueId pads to two digits and skips names already taken", () => {
  expect(uniqueId("web", new Set())).toBe("web-01");
  expect(uniqueId("web", new Set(["web-01"]))).toBe("web-02");
  expect(uniqueId("web", new Set(["web-01", "web-02"]))).toBe("web-03");
});

test("createEmptyAssembly uses the given name and a compact layout", () => {
  const a = createEmptyAssembly("shop");
  expect(a.metadata.name).toBe("shop");
  expect(a.spec.modules).toEqual([]);
  expect(a.spec.connections).toEqual([]);
  expect(a.editor.density).toBe("compact");
  expect(createEmptyAssembly().metadata.name).toBe("untitled-assembly");
});
