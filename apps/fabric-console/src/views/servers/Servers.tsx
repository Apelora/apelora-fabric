import type { WhoAmI } from "@apelora/api-client";

import { MachinesTab } from "../Membership";
import { PreflightView } from "../Preflight";
import { CertificatesTab, SettingsTab, UpgradesTab } from "./ClusterTabs";
import { MembersTab } from "./Members";
import { SectorsTab } from "./Sectors";

/** The Servers area's tabs, in order; each is its own address (`#/servers/<tab>`). */
export const SERVER_TABS = [
  { id: "members", label: "Members" },
  { id: "sectors", label: "Sectors" },
  { id: "machines", label: "Machines and join tokens", needsAdmin: true },
  { id: "settings", label: "Cluster settings" },
  { id: "certificates", label: "Certificates" },
  { id: "upgrades", label: "Upgrades" },
  { id: "preflight", label: "Preflight" },
] as const;
export type ServerTab = (typeof SERVER_TABS)[number]["id"];

export const isServerTab = (s: string | undefined): s is ServerTab => SERVER_TABS.some((t) => t.id === s);

/** The tabs this caller is offered (machines need `manage-infrastructure`). */
export const serverTabs = (me: WhoAmI | null) =>
  SERVER_TABS.filter((t) => !("needsAdmin" in t) || !me || me.permissions.includes("manage-infrastructure"));

export function ServersView({ tab, me, onAuth }: { tab: ServerTab; me: WhoAmI | null; onAuth: (m: string) => void }) {
  const tabs = serverTabs(me);
  const current = tabs.some((t) => t.id === tab) ? tab : "members";
  return (
    <div className="flex flex-col gap-6">
      {/* Links, not ARIA tabs: each tab is its own address, so Back and shared links work. */}
      <nav aria-label="Servers" className="tabs tabs-border tabs-sm flex-wrap border-b border-base-300" data-testid="server-tabs">
        {tabs.map((t) => (
          <a key={t.id} href={`#/servers/${t.id}`} className={`tab whitespace-nowrap ${current === t.id ? "tab-active" : "text-base-content/75"}`} aria-current={current === t.id ? "page" : undefined}>{t.label}</a>
        ))}
      </nav>
      <div key={current}>
        {current === "members" && <MembersTab me={me} onAuth={onAuth} />}
        {current === "sectors" && <SectorsTab me={me} onAuth={onAuth} />}
        {current === "machines" && <MachinesTab me={me} onAuth={onAuth} />}
        {current === "settings" && <SettingsTab me={me} onAuth={onAuth} />}
        {current === "certificates" && <CertificatesTab onAuth={onAuth} />}
        {current === "upgrades" && <UpgradesTab me={me} onAuth={onAuth} />}
        {current === "preflight" && <PreflightView onAuth={onAuth} level={2} />}
      </div>
    </div>
  );
}
