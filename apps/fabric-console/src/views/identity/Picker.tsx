import { useId, type ReactNode } from "react";

import type { Optional } from "../../lib/identity";

/**
 * A labelled choice from a list the API provides. While it loads, when it failed, and when this
 * caller may not list it, the control says so instead of showing an empty list.
 */
export function Picker<T>({ label, data, value, onChange, id, text, placeholder = "Choose…", offMessage, emptyMessage, testId, children }: {
  label: string;
  data: Optional<T[]>;
  value: string;
  onChange: (value: string) => void;
  id: (item: T) => string;
  text: (item: T) => string;
  placeholder?: string;
  offMessage: string;
  emptyMessage: string;
  testId?: string;
  children?: ReactNode;
}) {
  const selectId = useId();
  const noteId = useId();
  const note =
    data.kind === "loading" ? "Loading…"
      : data.kind === "error" ? `Could not load the list: ${data.message}`
        : data.kind === "off" ? offMessage
          : data.data.length === 0 ? emptyMessage
            : null;
  return (
    <div className="flex flex-col gap-1 text-sm">
      <label htmlFor={selectId}>{label}</label>
      <select id={selectId} className="select select-sm w-full" value={value} onChange={(e) => onChange(e.target.value)} disabled={data.kind !== "ok" || data.data.length === 0} aria-describedby={note ? noteId : undefined} data-testid={testId}>
        <option value="">{placeholder}</option>
        {data.kind === "ok" && data.data.map((item) => <option key={id(item)} value={id(item)}>{text(item)}</option>)}
      </select>
      {note && <span id={noteId} className={`text-xs ${data.kind === "error" ? "text-error" : "text-base-content/60"}`}>{note}</span>}
      {children}
    </div>
  );
}
