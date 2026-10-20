import { useMemo, useState } from "react";

import { iconChoices } from "../../features/applications/icons";
import { AppIcon } from "./common";

/** Pick an icon from the ones the editor already uses (appliance logos and generic shapes). */
export function IconPicker({ value, onChange, idPrefix = "icon" }: { value: string; onChange: (key: string) => void; idPrefix?: string }) {
  const [q, setQ] = useState("");
  const all = useMemo(iconChoices, []);
  const shown = useMemo(() => {
    const n = q.trim().toLowerCase();
    return n ? all.filter((c) => c.label.toLowerCase().includes(n) || c.key.includes(n)) : all;
  }, [all, q]);
  return (
    <div className="flex flex-col gap-2" data-testid="icon-picker">
      <input className="input input-xs w-full" placeholder="Search icons" aria-label="Search icons" value={q} onChange={(e) => setQ(e.target.value)} />
      <div role="radiogroup" aria-label="Application icon" className="grid max-h-40 grid-cols-8 gap-1.5 overflow-y-auto rounded-box border border-base-300 p-2">
        {shown.map((c) => (
          <button key={c.key} type="button" role="radio" aria-checked={value === c.key} aria-label={c.label} title={c.label} data-testid={`${idPrefix}-${c.key}`} data-selected={value === c.key}
            className={`rounded-lg p-0.5 transition-colors ${value === c.key ? "ring-2 ring-base-content" : "hover:bg-base-200"}`} onClick={() => onChange(c.key)}>
            <AppIcon icon={c.key} size={34} />
          </button>
        ))}
        {shown.length === 0 && <p className="col-span-8 py-3 text-center text-xs text-base-content/60">No icon matches.</p>}
      </div>
    </div>
  );
}
