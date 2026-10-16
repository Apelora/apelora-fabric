/** RJ45-style terminal: a socket (hollow, with contact pins) for inputs and a plug (solid) for outputs. */
export function Jack({ kind, className = "" }: { kind: "input" | "output"; className?: string }) {
  const body = "M0.75 2.25H3.25V0.75H6.75V2.25H9.25V9.25H0.75Z";
  return (
    <svg className={`apl-jack ${className}`} data-kind={kind} viewBox="0 0 10 10" aria-hidden="true" focusable="false">
      <path d={body} fill={kind === "output" ? "currentColor" : "var(--color-base-100)"} stroke="currentColor" strokeWidth="1" strokeLinejoin="round" />
      <path d="M2.6 5V8M4.2 5V8M5.8 5V8M7.4 5V8" stroke={kind === "output" ? "var(--color-base-100)" : "currentColor"} strokeWidth="0.8" strokeLinecap="round" opacity={kind === "output" ? 0.85 : 0.6} />
    </svg>
  );
}
