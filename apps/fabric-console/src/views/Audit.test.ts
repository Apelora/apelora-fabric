import { expect, test } from "vitest";

import { filtersFromHash, hashFor } from "./Audit";

test("filtersFromHash reads the query and drops unknown outcomes and invalid times", () => {
  expect(filtersFromHash("#/audit")).toEqual({
    actor: "",
    target: "",
    action: "",
    outcome: "",
    from: "",
    to: "",
  });
  expect(filtersFromHash("#/audit?actor=ada&outcome=denied&from=2026-01-01T00:00:00Z")).toMatchObject({
    actor: "ada",
    outcome: "denied",
    from: "2026-01-01T00:00:00Z",
  });
  expect(filtersFromHash("#/audit?outcome=nope&from=not-a-date").outcome).toBe("");
  expect(filtersFromHash("#/audit?from=not-a-date").from).toBe("");
  expect(filtersFromHash("#/audit?actor=" + "x".repeat(250)).actor).toHaveLength(200);
});

test("hashFor omits empty filters", () => {
  expect(hashFor("audit", { actor: "", target: "", action: "", outcome: "", from: "", to: "" })).toBe("#/audit");
  expect(hashFor("audit", { actor: "ada", target: "", action: "", outcome: "denied", from: "", to: "" })).toBe(
    "#/audit?actor=ada&outcome=denied",
  );
});
