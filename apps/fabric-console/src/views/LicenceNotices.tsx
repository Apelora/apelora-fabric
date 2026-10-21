// SPDX-License-Identifier: AGPL-3.0-only
import { Notice, Section, SectionHeader } from "../components/ui";
import { useLoad } from "../lib/api";

/** `/licenses.json`, made at build time by scripts/licenses/notices.mjs and served with this app. */
export type LicenceNotices = {
  product: string;
  licence: string;
  version: string;
  commit: string | null;
  sourceUrl: string;
  licenceUrl: string;
  agplUrl: string;
  noticesUrl: string;
  generated: string;
  problems: string[];
  components: { ecosystem: "cargo" | "npm"; name: string; version: string; license: string }[];
};

const plural = (n: number, what: string) => `${n} ${what}${n === 1 ? "" : "s"}`;

async function loadNotices(): Promise<LicenceNotices> {
  const res = await fetch("/licenses.json", { headers: { accept: "application/json" } });
  if (!res.ok) throw new Error(`The licence notices could not be read (HTTP ${res.status}). They are made when the Console is built.`);
  return (await res.json()) as LicenceNotices;
}

/** Administration → Licences: Apelora's licence, where its source is, and third-party notices. */
export function LicenceNoticesSection({ onAuth }: { onAuth: (m: string) => void }) {
  const [state, reload] = useLoad<LicenceNotices>(loadNotices, onAuth);
  return (
    <Section title="Licences" state={state} testId="licences" onRetry={reload}>
      {(n) => {
        const count = (eco: string) => n.components.filter((c) => c.ecosystem === eco).length;
        return (
          <>
            <SectionHeader title="Licences" />
            <dl className="divide-y divide-base-300 rounded-box border border-base-300 text-sm">
              <div className="flex flex-col gap-1 px-4 py-3 sm:flex-row sm:justify-between sm:gap-6">
                <dt className="font-medium">{n.product} licence</dt>
                <dd className="text-base-content/70 sm:text-right" data-testid="licence-own">{n.licence}</dd>
              </div>
              <div className="flex flex-col gap-1 px-4 py-3 sm:flex-row sm:justify-between sm:gap-6">
                <dt className="font-medium">This build</dt>
                <dd className="text-base-content/70 sm:text-right" data-testid="licence-build">Version {n.version}{n.commit ? `, commit ${n.commit.slice(0, 12)}` : ""}</dd>
              </div>
              <div className="flex flex-col gap-1 px-4 py-3 sm:flex-row sm:justify-between sm:gap-6">
                <dt className="font-medium">Source code</dt>
                <dd className="break-all sm:text-right">{n.sourceUrl ? <a className="link" href={n.sourceUrl} data-testid="licence-source">{n.sourceUrl}</a> : <span className="text-base-content/70">Not set at build time (APELORA_SOURCE_URL)</span>}</dd>
              </div>
              <div className="flex flex-col gap-1 px-4 py-3 sm:flex-row sm:justify-between sm:gap-6">
                <dt className="font-medium">Third-party software</dt>
                <dd className="text-base-content/70 sm:text-right" data-testid="licence-counts">{plural(count("cargo"), "Rust crate")} and {plural(count("npm"), "npm package")}</dd>
              </div>
            </dl>
            {n.problems.length > 0 && <Notice tone="warn" testId="licence-problems">The list is incomplete: {n.problems.join("; ")}.</Notice>}
            <div className="flex flex-wrap gap-2">
              <a className="btn btn-sm btn-outline" href="/licenses" target="_blank" rel="noopener" data-testid="licence-page">Open the licence page</a>
              <a className="btn btn-sm btn-ghost" href={n.noticesUrl} target="_blank" rel="noopener">Third-party licence texts</a>
            </div>
          </>
        );
      }}
    </Section>
  );
}
