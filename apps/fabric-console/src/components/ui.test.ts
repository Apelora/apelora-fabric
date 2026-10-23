import { expect, test } from "vitest";

import { sentence } from "./ui";

test("sentence capitalises a clause the API returned in lower case", () => {
  expect(sentence("not signed in")).toBe("Not signed in");
  expect(sentence("Already capital")).toBe("Already capital");
  expect(sentence("")).toBe("");
});
