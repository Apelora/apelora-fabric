// @vitest-environment jsdom
import { afterEach, expect, test, vi } from "vitest";

import { messageOf, readToken, writeToken } from "./api";

afterEach(() => {
  sessionStorage.clear();
  localStorage.clear();
  vi.unstubAllGlobals();
});

test("messageOf prefers Error.message and stringifies anything else", () => {
  expect(messageOf(new Error("nope"))).toBe("nope");
  expect(messageOf("plain")).toBe("plain");
  expect(messageOf(404)).toBe("404");
});

test("the API token lives in sessionStorage and never in localStorage", () => {
  expect(readToken()).toBeNull();
  writeToken("apt_secret");
  expect(readToken()).toBe("apt_secret");
  expect(sessionStorage.getItem("apelora.apiToken")).toBe("apt_secret");
  expect(localStorage.getItem("apelora.apiToken")).toBeNull();
  writeToken(null);
  expect(readToken()).toBeNull();
  expect(sessionStorage.getItem("apelora.apiToken")).toBeNull();
});
