import { expect, test } from "vitest";

import { bytes } from "./InstanceDetail";

test("bytes uses binary units and an em dash when the size is missing", () => {
  expect(bytes()).toBe("—");
  expect(bytes(null)).toBe("—");
  expect(bytes(0)).toBe("0 B");
  expect(bytes(512)).toBe("512 B");
  expect(bytes(1024)).toBe("1.0 KiB");
  expect(bytes(1536)).toBe("1.5 KiB");
  expect(bytes(1024 ** 2)).toBe("1.0 MiB");
  expect(bytes(1024 ** 3)).toBe("1.0 GiB");
});
