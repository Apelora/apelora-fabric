// SPDX-License-Identifier: AGPL-3.0-only
// Writes the release bundle's software inventory: every Rust crate built into the two services
// and every npm package shipped in the two web apps, with version, licence and source. Uses only
// what the licence checks already use (cargo metadata, npm ls); no extra tools.
// Usage: node scripts/release/inventory.mjs <output directory>
// The same functions feed the licence notices the web apps serve (scripts/licenses/notices.mjs).
/* global console, process */
import { execFileSync } from "node:child_process";
import { readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

/** The repository root (two levels above this file). */
export const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

const run = (cmd, args) => execFileSync(cmd, args, { encoding: "utf8", maxBuffer: 256 << 20, cwd: ROOT, stdio: ["ignore", "pipe", "pipe"] });

/** Licence and notice files in a package directory (LICENSE, LICENCE, COPYING, NOTICE, with any suffix). */
export function licenceFiles(dir) {
  try {
    return readdirSync(dir)
      .filter((f) => /^(licen[cs]e|copying|notice)([-._].*)?$/i.test(f))
      .sort()
      .map((f) => join(dir, f));
  } catch {
    return [];
  }
}

/**
 * Rust: the crates the two services depend on for the build host (normal and build edges).
 * Each entry has `dir`, the crate's source directory (for its licence files).
 */
export function cargoComponents() {
  const host = run("rustc", ["-vV"]).match(/^host: (.+)$/m)?.[1];
  const meta = JSON.parse(run("cargo", ["metadata", "--format-version", "1", "--locked", ...(host ? ["--filter-platform", host] : [])]));
  const byId = new Map(meta.packages.map((p) => [p.id, p]));
  const nodes = new Map(meta.resolve.nodes.map((n) => [n.id, n]));
  const roots = meta.packages.filter((p) => ["apelora-control-plane", "apelora-deployment-worker"].includes(p.name)).map((p) => p.id);
  const seen = new Set();
  const stack = [...roots];
  while (stack.length) {
    const id = stack.pop();
    if (seen.has(id)) continue;
    seen.add(id);
    for (const d of nodes.get(id)?.deps ?? []) {
      if (d.dep_kinds.some((k) => k.kind === null || k.kind === "build")) stack.push(d.pkg);
    }
  }
  return [...seen].map((id) => byId.get(id)).filter(Boolean).map((p) => ({
    ecosystem: "cargo",
    name: p.name,
    version: p.version,
    license: p.license ?? (p.license_file ? `see ${p.license_file.split("/").pop()}` : "UNKNOWN"),
    source: p.source ?? "this repository",
    dir: dirname(p.manifest_path),
  })).sort((a, b) => a.name.localeCompare(b.name) || a.version.localeCompare(b.version));
}

/** npm: production dependencies of the workspaces (what the web apps bundle), as check-npm.mjs reads them. */
export function npmComponents() {
  const tree = JSON.parse(run("npm", ["ls", "--omit=dev", "--all", "--json"]));
  const pkgs = new Map();
  (function walk(deps) {
    for (const [name, d] of Object.entries(deps ?? {})) {
      if (d.version && !pkgs.has(`${name}@${d.version}`)) {
        let dir = join(ROOT, "node_modules", name);
        let license = "UNKNOWN";
        const ours = name.startsWith("@apelora/");
        try {
          const pkg = JSON.parse(readFileSync(join(dir, "package.json"), "utf8"));
          license = typeof pkg.license === "string" ? pkg.license : (pkg.license?.type ?? "UNKNOWN");
          // A different version hoisted to the root: its licence files are not this one's.
          if (pkg.version !== d.version) dir = null;
        } catch {
          dir = null;
          if (ours) license = "AGPL-3.0-only";
        }
        pkgs.set(`${name}@${d.version}`, { ecosystem: "npm", name, version: d.version, license, source: ours ? "this repository" : (d.resolved ?? "registry"), dir });
      }
      walk(d.dependencies);
    }
  })(tree.dependencies);
  return [...pkgs.values()].sort((a, b) => a.name.localeCompare(b.name) || a.version.localeCompare(b.version));
}

// Command line: the release bundle's inventory files.
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const out = process.argv[2];
  if (!out) {
    console.error("usage: node scripts/release/inventory.mjs <output directory>");
    process.exit(2);
  }
  const generated = new Date().toISOString();
  const strip = (list) => list.map((c) => ({ ecosystem: c.ecosystem, name: c.name, version: c.version, license: c.license, source: c.source }));
  const crates = strip(cargoComponents());
  const npm = strip(npmComponents());
  const doc = (components, what) => JSON.stringify({ format: "apelora-inventory/1", what, generated, components }, null, 2) + "\n";
  writeFileSync(join(out, "cargo.json"), doc(crates, "Rust crates in apelora-control-plane and apelora-deployment-worker"));
  writeFileSync(join(out, "npm.json"), doc(npm, "npm packages shipped in the Studio and the Fabric Console"));
  console.log(`inventory: ${crates.length} crates, ${npm.length} npm packages`);
}
