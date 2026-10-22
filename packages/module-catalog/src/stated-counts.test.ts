import { readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

import { MODULE_CLASSES } from "./classes";
import { RUNTIMES } from "./runtimes";

/**
 * Counts written in repository pages (alert rules, module classes, deployable classes, API
 * operations) have to match the code. Generated pages are included: a stale catalog reference
 * fails here as well as in the generate step. Lessons learned quotes an old wrong count on
 * purpose, so that file is not read.
 */

const root = join(dirname(fileURLToPath(import.meta.url)), "../../..");
const WORDS = ["zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven", "twelve", "thirteen", "fourteen", "fifteen", "sixteen", "seventeen", "eighteen", "nineteen", "twenty"];
const COUNT = `(?:${WORDS.join("|")}|\\d+)`;

const asCount = (raw: string): number => {
  const text = raw.toLowerCase();
  if (/^\d+$/.test(text)) return Number(text);
  const n = WORDS.indexOf(text);
  if (n < 0) throw new Error(`not a count: ${raw}`);
  return n;
};

const pages = (dir: string, out: string[] = []): string[] => {
  for (const name of readdirSync(dir)) {
    if (name === "LESSONS-LEARNED.md") continue;
    const path = join(dir, name);
    if (statSync(path).isDirectory()) pages(path, out);
    else if (name.endsWith(".md")) out.push(path);
  }
  return out;
};

const ruleCount = (): number => {
  const src = readFileSync(join(root, "crates/domain/src/alerts.rs"), "utf8");
  const found = src.match(/assert_eq!\(RULES\.len\(\), (\d+)\)/);
  if (!found?.[1]) throw new Error("alerts.rs does not assert RULES.len()");
  return Number(found[1]);
};

const operationCounts = (): { openapi: number; client: number } => {
  const openapi = readFileSync(join(root, "packages/api-client/openapi.json"), "utf8");
  const client = readFileSync(join(root, "packages/api-client/src/generated.ts"), "utf8");
  return {
    openapi: openapi.match(/"operationId":/g)?.length ?? 0,
    client: client.match(/^ {2}[A-Za-z0-9_]+: \{ method: /gm)?.length ?? 0,
  };
};

describe("stated counts", () => {
  it("matches built-in alert rules, module classes, deployable classes and API operations", () => {
    const rules = ruleCount();
    const modules = MODULE_CLASSES.length;
    const deployable = Object.keys(RUNTIMES).length;
    const ops = operationCounts();
    const problems: string[] = [];
    if (ops.openapi !== ops.client) problems.push(`OpenAPI has ${ops.openapi} operations and the generated client has ${ops.client}`);

    const expectCount = (label: string, file: string, raw: string, actual: number) => {
      const stated = asCount(raw);
      if (stated !== actual) problems.push(`${relative(root, file)}: ${label} says ${raw} (${stated}), the code has ${actual}`);
    };

    for (const file of pages(join(root, "docs"))) {
      const text = readFileSync(file, "utf8");
      for (const hit of text.matchAll(new RegExp(`(${COUNT})\\s+built-in rules`, "gi"))) expectCount("built-in rules", file, hit[1]!, rules);
      for (const hit of text.matchAll(new RegExp(`checks\\s+(${COUNT})\\s+rules\\b`, "gi"))) expectCount("rules checked", file, hit[1]!, rules);
      for (const hit of text.matchAll(/(\d+)\s+original module classes/g)) expectCount("module classes", file, hit[1]!, modules);
      for (const hit of text.matchAll(/^(\d+)\s+modules in /gm)) expectCount("modules", file, hit[1]!, modules);
      for (const hit of text.matchAll(/(\d+)\s+of\s+(\d+)\s+classes/g)) {
        expectCount("deployable classes", file, hit[1]!, deployable);
        expectCount("module classes", file, hit[2]!, modules);
      }
      for (const hit of text.matchAll(new RegExp(`,\\s+(${COUNT})\\s+deployable\\b`, "gi"))) expectCount("deployable classes", file, hit[1]!, deployable);
      for (const hit of text.matchAll(/the other (\d+) classes/g)) expectCount("design-only classes", file, hit[1]!, modules - deployable);
      for (const hit of text.matchAll(/(\d+)\s+API operations/g)) expectCount("API operations", file, hit[1]!, ops.openapi);
    }

    expect(problems, problems.join("\n")).toEqual([]);
    expect(rules).toBeGreaterThan(0);
    expect(modules).toBeGreaterThan(0);
    expect(ops.openapi).toBeGreaterThan(0);
  });
});
