import { expect, test } from "vitest";

import { DENSITY_SIZE, NODE, countPorts, effectiveSize, minHeightForPorts, minModuleHeight, portCenterY } from "./geometry";
import type { Port } from "./schema";

const port = (id: string, direction: Port["direction"]): Port => ({
  id,
  name: id,
  direction,
  type: "http",
  optional: false,
  multiplicity: "one",
});

test("compact nodes stay 128 by 96 until a side needs more port rows", () => {
  expect(DENSITY_SIZE.compact).toEqual({ width: 128, height: 96 });
  expect(DENSITY_SIZE.comfortable).toEqual({ width: 152, height: 112 });
  expect(DENSITY_SIZE.expanded).toEqual({ width: 178, height: 150 });
  expect(NODE.width).toBe(128);
  expect(NODE.height).toBe(96);
  expect(minModuleHeight(1, 1)).toBe(96);
  expect(minModuleHeight(3, 1)).toBe(60 + 18 * 3);
  expect(minHeightForPorts([port("a", "input"), port("b", "input"), port("c", "output")])).toBe(60 + 18 * 2);
});

test("countPorts splits the bus by direction", () => {
  expect(countPorts([])).toEqual({ inputs: 0, outputs: 0 });
  expect(countPorts([port("in", "input"), port("out", "output"), port("in2", "input")])).toEqual({
    inputs: 2,
    outputs: 1,
  });
});

test("effectiveSize never shrinks below the compact width or the port height", () => {
  const ports = [port("a", "input"), port("b", "input"), port("c", "input")];
  expect(effectiveSize({ width: 80, height: 40, custom: false }, ports, "compact")).toEqual({
    width: 128,
    height: minHeightForPorts(ports),
  });
  expect(effectiveSize({ width: 200, height: 200, custom: true }, [], "compact")).toEqual({
    width: 200,
    height: 200,
  });
});

test("portCenterY steps down the bus from the header", () => {
  expect(portCenterY(0)).toBe(NODE.header + 18);
  expect(portCenterY(1)).toBe(NODE.header + 18 + NODE.portPitch);
});
