// SPDX-License-Identifier: AGPL-3.0-only
// Checks the licences of the npm packages the apps ship (production dependencies of every
// workspace) against the same allow-list as deny.toml. Development tools are not shipped and
// are not checked. Exits non-zero naming each package whose licence is not allowed.
// Usage: node scripts/licenses/check-npm.mjs
/* global console, process */
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { join } from "node:path";

const ALLOWED = new Set([
  "Apache-2.0", "MIT", "MIT-0", "BSD-2-Clause", "BSD-3-Clause", "ISC", "Zlib", "BSL-1.0",
  "Unlicense", "CC0-1.0", "Unicode-3.0", "0BSD", "BlueOak-1.0.0",
]);

/** Whether an SPDX expression is satisfied by the allow-list (OR: any side; AND: both). */
export function allowed(expression) {
  const e = expression.trim().replace(/^\((.*)\)$/, "$1");
  if (/ OR /.test(e)) return e.split(/ OR /).some(allowed);
  if (/ AND /.test(e)) return e.split(/ AND /).every(allowed);
  return ALLOWED.has(e.replace(/\+$/, ""));
}

const tree = JSON.parse(execFileSync("npm", ["ls", "--omit=dev", "--all", "--json"], { encoding: "utf8", maxBuffer: 64 << 20 }));
const seen = new Map();
(function walk(deps, path) {
  for (const [name, d] of Object.entries(deps ?? {})) {
    if (!name.startsWith("@apelora/") && d.version && !seen.has(`${name}@${d.version}`)) {
      seen.set(`${name}@${d.version}`, path ? `${path} > ${name}` : name);
    }
    walk(d.dependencies, path ? `${path} > ${name}` : name);
  }
})(tree.dependencies, "");

const problems = [];
for (const [id, via] of seen) {
  const name = id.slice(0, id.lastIndexOf("@"));
  let license;
  try {
    const pkg = JSON.parse(readFileSync(join("node_modules", name, "package.json"), "utf8"));
    license = typeof pkg.license === "string" ? pkg.license : (pkg.license?.type ?? "UNKNOWN");
  } catch {
    // Not hoisted to the root: npm ls found it, so look it up through the workspace.
    license = "UNKNOWN (not found under node_modules)";
  }
  if (!allowed(license)) problems.push(`${id}: ${license} (via ${via})`);
}
console.log(`${seen.size} production packages checked`);
if (problems.length) {
  console.error(`Licences that need a maintainer decision:\n  ${problems.join("\n  ")}`);
  process.exit(1);
}
