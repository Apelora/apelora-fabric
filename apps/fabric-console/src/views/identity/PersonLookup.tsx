import { useId, useState } from "react";

import type { UserDto } from "@apelora/api-client";

import { Badge, Notice } from "../../components/ui";
import { client } from "../../lib/api";
import { personName, useRun } from "../../lib/identity";

const MIN_CHARS = 3;

/**
 * Finds one person by their exact user name or e-mail address. Someone who has never signed in
 * does not exist yet, so an empty answer says that instead of guessing.
 */
export function PersonLookup({ onAuth, label = "Find a person", actionLabel, onPick, testId = "person-lookup", hint }: {
  onAuth: (m: string) => void;
  label?: string;
  actionLabel: string;
  onPick: (user: UserDto) => void;
  testId?: string;
  hint?: string;
}) {
  const [q, setQ] = useState("");
  const [found, setFound] = useState<{ query: string; people: UserDto[] } | null>(null);
  const { busy, error, run } = useRun(onAuth);
  const inputId = useId();
  const hintId = useId();
  const ready = q.trim().length >= MIN_CHARS;
  const find = () => {
    if (!ready || busy) return;
    const query = q.trim();
    setFound(null);
    void run(async () => {
      const r = await client.request("lookup_user", { query: { q: query } });
      setFound({ query, people: r.items });
    });
  };
  return (
    <div className="flex flex-col gap-2" role="search" aria-label={label} data-testid={testId}>
      <label htmlFor={inputId} className="text-sm font-medium">{label}</label>
      <div className="flex flex-wrap items-center gap-2">
        <input
          id={inputId}
          className="input input-sm w-full max-w-sm"
          value={q}
          onChange={(e) => { setQ(e.target.value); setFound(null); }}
          onKeyDown={(e) => { if (e.key === "Enter") { e.preventDefault(); find(); } }}
          placeholder="Exact user name or e-mail address"
          autoComplete="off"
          spellCheck={false}
          aria-describedby={hintId}
          data-testid={`${testId}-input`}
        />
        <button type="button" className="btn btn-sm btn-outline" disabled={!ready || busy} onClick={find} data-testid={`${testId}-find`}>{busy ? "Searching…" : "Find"}</button>
      </div>
      <p id={hintId} className="text-xs text-base-content/60">{hint ?? `Type at least ${MIN_CHARS} characters of the exact user name or e-mail address. People appear here after their first sign-in.`}</p>
      {error && <Notice tone="error" testId={`${testId}-error`}>{error}</Notice>}
      {found && found.people.length === 0 && (
        <Notice tone="info" testId={`${testId}-none`}>No one has signed in with the user name or e-mail “{found.query}”. Ask them to sign in once, then look again.</Notice>
      )}
      {found && found.people.length > 0 && (
        <ul className="divide-y divide-base-300 rounded-box border border-base-300" aria-label="People found">
          {found.people.map((u) => (
            <li key={u.id} className="flex flex-wrap items-center justify-between gap-2 px-3 py-2 text-sm" data-testid={`${testId}-result`}>
              <span className="min-w-0">
                <span className="font-medium">{personName(u)}</span>
                <span className="block truncate text-xs text-base-content/60">{[u.username, u.email].filter(Boolean).join(" · ") || u.id}</span>
              </span>
              <span className="flex items-center gap-2">
                {u.disabled && <Badge tone="error">Disabled</Badge>}
                <button type="button" className="btn btn-xs btn-outline" disabled={u.disabled} onClick={() => onPick(u)} aria-label={`${actionLabel}: ${personName(u)}`}>{actionLabel}</button>
              </span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
