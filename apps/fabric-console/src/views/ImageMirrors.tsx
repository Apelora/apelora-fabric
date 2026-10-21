// SPDX-License-Identifier: AGPL-3.0-only
import type { ClassImage, CoverageKind, ImageMirrorsReport, ProbeState } from "@apelora/api-client";

import { Badge, Section, SectionHeader, type Tone } from "../components/ui";
import { client, useLoad } from "../lib/api";

const PROBE: Record<ProbeState, { label: string; tone: Tone }> = {
  answers: { label: "Answers", tone: "ok" },
  fails: { label: "Does not answer", tone: "error" },
  "not-checked": { label: "Not checked", tone: "neutral" },
};

const PROTOCOL = { simplestreams: "system and VM images", oci: "OCI images" } as const;

/** "NGINX (channel stable, mainline)": each class once, with the settings that pick its images. */
function usedBy(images: ClassImage[]): string {
  const by = new Map<string, string[]>();
  for (const i of images) {
    const list = by.get(i.displayName) ?? [];
    if (i.setting) list.push(i.setting);
    by.set(i.displayName, list);
  }
  return [...by].map(([name, settings]) => {
    const [first] = settings;
    if (first === undefined) return name;
    const property = first.split(" = ")[0];
    return `${name} (${property} ${settings.map((s) => s.split(" = ")[1] ?? s).join(", ")})`;
  }).join(", ");
}

function coverage(kind: CoverageKind, mirror: string | null | undefined, anyMirrors: boolean): { label: string; tone: Tone; detail: string } {
  if (kind === "mirrored") return { label: "Mirrored", tone: "ok", detail: `from ${mirror ?? "a mirror"}` };
  if (kind === "refused") return { label: "Refused", tone: "error", detail: "no mirror: deployments that need it fail" };
  return { label: "From upstream", tone: anyMirrors ? "warn" : "neutral", detail: "no mirror: pulled directly" };
}

function modeLine(r: ImageMirrorsReport): { tone: Tone; text: string } {
  if (r.mode === "mirrors-only") return { tone: "ok", text: "Only mirrors may be used." };
  if (r.mirrors.length) return { tone: "warn", text: "Other image servers are still allowed. Without internet access, allow mirrors only." };
  return { tone: "neutral", text: "No mirrors: images come from their upstream servers. Without internet access, configure mirrors." };
}

const time = (t: string | null | undefined) => (t ? new Date(t).toLocaleTimeString() : null);

export function ImageMirrorsSection({ onAuth }: { onAuth: (m: string) => void }) {
  const [report, reload] = useLoad<ImageMirrorsReport>(() => client.get("image_mirrors"), onAuth);
  return (
    <Section title="Image mirrors" state={report} testId="image-mirrors" onRetry={reload}>
      {(r) => {
        const mode = modeLine(r);
        return (
          <div className="flex flex-col gap-4">
            <div className="flex flex-col gap-2">
              <SectionHeader title="Image mirrors">
                <button type="button" className="btn btn-sm btn-ghost" onClick={reload} data-testid="mirrors-recheck">Check again</button>
              </SectionHeader>
              <p className="flex items-start gap-2 text-sm text-base-content/70" data-testid="mirrors-mode">
                <span className={`status mt-1.5 shrink-0 ${mode.tone === "ok" ? "status-success" : mode.tone === "warn" ? "status-warning" : "status-neutral"}`} aria-hidden="true" />
                <span>{mode.text}</span>
              </p>
            </div>

            {r.mirrors.length > 0 && (
              <div className="flex flex-col gap-2">
                <h3 className="text-sm font-semibold">Mirrors</h3>
                <ul className="divide-y divide-base-300 rounded-box border border-base-300">
                  {r.mirrors.map((m) => (
                    <li key={m.mirror} className="flex flex-col gap-1 px-4 py-3 sm:flex-row sm:items-start sm:justify-between sm:gap-6" data-testid="mirror-row">
                      <span className="flex min-w-0 flex-col text-sm">
                        <span className="break-all font-mono font-medium">{m.mirror}</span>
                        <span className="break-all text-base-content/60">for {m.upstream}</span>
                      </span>
                      <span className="flex flex-col gap-1 text-sm text-base-content/70 sm:items-end sm:text-right">
                        <Badge tone={PROBE[m.probe.state].tone}>{PROBE[m.probe.state].label}</Badge>
                        <span>{m.probe.detail}{time(m.checkedAt) && ` · checked ${time(m.checkedAt)}`}</span>
                      </span>
                    </li>
                  ))}
                </ul>
              </div>
            )}

            <div className="flex flex-col gap-2">
              <h3 className="text-sm font-semibold">Image servers the catalog uses</h3>
              {r.servers.length === 0 ? (
                <p className="text-sm text-base-content/70">No deployable class names an image server.</p>
              ) : (
                <ul className="divide-y divide-base-300 rounded-box border border-base-300">
                  {r.servers.map((s) => {
                    const c = coverage(s.coverage, s.mirror, r.mirrors.length > 0);
                    return (
                      <li key={`${s.protocol} ${s.server}`} className="flex flex-col gap-1 px-4 py-3 sm:flex-row sm:items-start sm:justify-between sm:gap-6" data-testid="image-server-row">
                        <span className="flex min-w-0 flex-col text-sm">
                          <span className="break-all font-mono font-medium">{s.server}</span>
                          <span className="text-base-content/60">{PROTOCOL[s.protocol]} for {usedBy(s.images)}</span>
                        </span>
                        <span className="flex flex-col gap-1 text-sm text-base-content/70 sm:max-w-[45%] sm:items-end sm:text-right">
                          <Badge tone={c.tone}>{c.label}</Badge>
                          <span className="break-all">{c.detail}</span>
                        </span>
                      </li>
                    );
                  })}
                </ul>
              )}
              <p className="text-xs text-base-content/60">
                Read-only: set APELORA_IMAGE_MIRRORS and APELORA_IMAGE_SERVERS alike for the control plane and the worker. Mirrors are checked at most once a minute.
              </p>
            </div>
          </div>
        );
      }}
    </Section>
  );
}
