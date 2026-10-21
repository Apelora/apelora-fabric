// SPDX-License-Identifier: AGPL-3.0-only
// Licence notices served with the Studio and the Fabric Console, made at build time from the
// same sources as the release inventory (cargo metadata, npm ls; scripts/release/inventory.mjs):
//
//   /licenses                   a page: Apelora's licence, where to get its source (the AGPL's
//                               network clause), and every third-party package with its licence
//   /licenses.json              the same as data (the Console's Administration page reads it)
//   /third-party-licenses.txt   the licence and notice files of every third-party package
//   /apelora-license.txt        LICENSE and COPYRIGHT.md as they are in the repository
//
// Everything is served from the apps' own origin, so it works without internet access.
// APELORA_SOURCE_URL (at build time) names where this build's source is offered; it defaults to
// the repository in CITATION.cff. Change it when you run a modified Apelora: the AGPL asks you to
// offer *your* source.
/* global process, console */
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { ROOT, cargoComponents, licenceFiles, npmComponents } from "../release/inventory.mjs";

const PRODUCT = "Apelora";
const LICENCE = "AGPL-3.0-only";
const AGPL_URL = "https://www.gnu.org/licenses/agpl-3.0.html";

const escape = (s) => String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);

function sourceUrl() {
  const fromEnv = process.env.APELORA_SOURCE_URL?.trim();
  if (fromEnv) return fromEnv;
  const cff = readFileSync(join(ROOT, "CITATION.cff"), "utf8");
  return cff.match(/^repository-code:\s*"?([^"\n]+)"?/m)?.[1]?.trim() ?? "";
}

function version() {
  const cargo = readFileSync(join(ROOT, "Cargo.toml"), "utf8");
  return cargo.match(/\[workspace\.package\][^[]*?\nversion\s*=\s*"([^"]+)"/)?.[1] ?? "unknown";
}

function commit() {
  try {
    return execFileSync("git", ["rev-parse", "HEAD"], { cwd: ROOT, encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }).trim();
  } catch {
    return null;
  }
}

/** The licence and notice texts of a package directory, joined. */
function texts(dir) {
  if (!dir) return "";
  return licenceFiles(dir)
    .map((f) => {
      try {
        return readFileSync(f, "utf8").replace(/\r\n/g, "\n").trim();
      } catch {
        return "";
      }
    })
    .filter(Boolean)
    .join("\n\n");
}

/**
 * Builds the notices. With `strict`, a part that cannot be read (no Rust toolchain, say) is an
 * error; without, it is reported in `problems` and on the page (the development server).
 */
export function buildNotices({ strict = false } = {}) {
  const problems = [];
  const read = (what, f) => {
    try {
      return f();
    } catch (e) {
      const message = `${what}: ${(e instanceof Error ? e.message : String(e)).split("\n")[0]}`;
      if (strict) throw new Error(`licence notices: ${message}`, { cause: e });
      problems.push(message);
      return [];
    }
  };
  const all = [...read("Rust crates (cargo metadata)", cargoComponents), ...read("npm packages (npm ls)", npmComponents)];
  const third = all.filter((c) => c.source !== "this repository");
  // Identical licence texts are printed once, with every package that carries them.
  const groups = new Map();
  for (const c of third) {
    const text = texts(c.dir);
    if (!text) continue;
    const key = createHash("sha256").update(text).digest("hex");
    if (!groups.has(key)) groups.set(key, { text, users: [] });
    groups.get(key).users.push(`${c.name} ${c.version}`);
  }
  const withText = new Set([...groups.values()].flatMap((g) => g.users));
  const data = {
    format: "apelora-licenses/1",
    product: PRODUCT,
    licence: LICENCE,
    version: version(),
    commit: commit(),
    sourceUrl: sourceUrl(),
    licenceUrl: "/apelora-license.txt",
    agplUrl: AGPL_URL,
    noticesUrl: "/third-party-licenses.txt",
    generated: new Date().toISOString(),
    problems,
    components: third.map((c) => ({ ecosystem: c.ecosystem, name: c.name, version: c.version, license: c.license, source: c.source, noticeIncluded: withText.has(`${c.name} ${c.version}`) })),
  };
  const json = JSON.stringify(data, null, 2) + "\n";

  const txt = [
    `Third-party software in ${PRODUCT} ${data.version}${data.commit ? ` (${data.commit.slice(0, 12)})` : ""}`,
    `Generated ${data.generated} from cargo metadata and npm. Each block is a licence or notice file,`,
    "followed by the packages that ship it. Packages without a licence file in their source are",
    "listed on /licenses with their declared licence.",
    "",
    ...[...groups.values()].sort((a, b) => a.users[0].localeCompare(b.users[0])).flatMap((g) => [
      "=".repeat(78),
      `Used by: ${g.users.sort().join(", ")}`,
      "-".repeat(78),
      g.text,
      "",
    ]),
  ].join("\n");

  const own = [readFileSync(join(ROOT, "LICENSE"), "utf8"), "", "=".repeat(78), "", readFileSync(join(ROOT, "COPYRIGHT.md"), "utf8")].join("\n");

  const rows = (eco) =>
    third
      .filter((c) => c.ecosystem === eco)
      .map((c) => `<tr><td>${escape(c.name)}</td><td>${escape(c.version)}</td><td>${escape(c.license)}</td></tr>`)
      .join("\n");
  const count = (eco) => third.filter((c) => c.ecosystem === eco).length;
  const source = data.sourceUrl
    ? `<a href="${escape(data.sourceUrl)}">${escape(data.sourceUrl)}</a>`
    : "ask the administrator of this installation";
  const html = `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Licences · ${PRODUCT}</title>
<link rel="icon" type="image/svg+xml" href="/logo.svg">
<link rel="stylesheet" href="/licenses.css">
</head>
<body>
<main>
<h1>Licences</h1>
<section aria-labelledby="own">
<h2 id="own">${PRODUCT}</h2>
<p>${PRODUCT} is free software under the GNU Affero General Public License, version 3 only (${LICENCE}).</p>
<p>You use it over a network, so that licence gives you the right to get its complete source code. Get the source of version ${escape(data.version)}${data.commit ? `, commit <code>${escape(data.commit)}</code>` : ""}, at ${source}.</p>
<ul>
<li><a href="/apelora-license.txt">Licence and copyright files of this build</a></li>
<li><a href="${AGPL_URL}">The GNU Affero General Public License, version 3</a> (gnu.org)</li>
</ul>
</section>
<section aria-labelledby="third">
<h2 id="third">Third-party software</h2>
<p>${PRODUCT} includes ${count("cargo")} Rust crates and ${count("npm")} npm packages written by others, each under its own licence. <a href="/third-party-licenses.txt">Read their licence and notice texts</a>.</p>
${problems.length ? `<p class="problem">This list is incomplete: ${problems.map(escape).join("; ")}.</p>` : ""}
<h3>Services (Rust crates)</h3>
<div class="table"><table><thead><tr><th>Package</th><th>Version</th><th>Licence</th></tr></thead><tbody>
${rows("cargo")}
</tbody></table></div>
<h3>Web applications (npm packages)</h3>
<div class="table"><table><thead><tr><th>Package</th><th>Version</th><th>Licence</th></tr></thead><tbody>
${rows("npm")}
</tbody></table></div>
</section>
<p class="generated">Generated ${escape(data.generated)}.</p>
</main>
</body>
</html>
`;
  const css = `:root{color-scheme:light;font-family:ui-sans-serif,system-ui,-apple-system,"Segoe UI",sans-serif;color:#1f2328;background:#fff}
body{margin:0}
main{max-width:56rem;margin:0 auto;padding:2rem 1rem 3rem;line-height:1.5}
h1{font-size:1.5rem;margin:0 0 1.5rem}
h2{font-size:1.125rem;margin:2rem 0 .5rem}
h3{font-size:1rem;margin:1.5rem 0 .5rem}
p,li{font-size:.9375rem;max-width:42rem}
a{color:#0b57d0}
code{font-size:.8125rem;word-break:break-all}
.table{overflow-x:auto;border:1px solid #d0d7de;border-radius:12px}
table{border-collapse:collapse;width:100%;font-size:.875rem}
th,td{text-align:left;padding:.375rem .75rem;border-bottom:1px solid #eaeef2;overflow-wrap:anywhere}
th{background:#f7f7f8;font-weight:600}
tr:last-child td{border-bottom:0}
.problem{color:#9a3412}
.generated{color:#59636e;font-size:.8125rem;margin-top:2rem}
`;
  return {
    data,
    files: {
      "licenses.json": { type: "application/json; charset=utf-8", body: json },
      "licenses.html": { type: "text/html; charset=utf-8", body: html },
      "licenses.css": { type: "text/css; charset=utf-8", body: css },
      "third-party-licenses.txt": { type: "text/plain; charset=utf-8", body: txt },
      "apelora-license.txt": { type: "text/plain; charset=utf-8", body: own },
    },
  };
}

/**
 * Vite plugin: serves the notices from the development server (made on first request) and adds
 * them to every production build (made once per build; a part that cannot be read fails it).
 */
export function licenseNotices() {
  let dev = null;
  return {
    name: "apelora-license-notices",
    configureServer(server) {
      server.middlewares.use((req, res, next) => {
        const path = (req.url ?? "").split("?")[0];
        const name = path === "/licenses" ? "licenses.html" : path.slice(1);
        if (!["licenses.html", "licenses.json", "licenses.css", "third-party-licenses.txt", "apelora-license.txt"].includes(name)) return next();
        dev ??= buildNotices({ strict: false });
        const f = dev.files[name];
        res.setHeader("content-type", f.type);
        res.end(f.body);
      });
    },
    generateBundle() {
      const { files, data } = buildNotices({ strict: true });
      for (const [fileName, f] of Object.entries(files)) this.emitFile({ type: "asset", fileName, source: f.body });
      console.log(`licence notices: ${data.components.length} third-party packages`);
    },
  };
}
