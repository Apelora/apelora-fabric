import { cableColor, cableOutline } from "./cables";

/** RJ45-style socket: hollow, with a latch notch and contact pins. Every terminal is a socket; cables plug into them. */
export function Jack({ kind, className = "" }: { kind: "input" | "output"; className?: string }) {
  return (
    <svg className={`apl-jack ${className}`} data-kind={kind} viewBox="0 0 10 10" aria-hidden="true" focusable="false">
      <path d="M0.75 2.25H3.25V0.75H6.75V2.25H9.25V9.25H0.75Z" fill="var(--color-base-100)" stroke="currentColor" strokeWidth="1" strokeLinejoin="round" />
      <path d="M2.6 5V8M4.2 5V8M5.8 5V8M7.4 5V8" stroke="currentColor" strokeWidth="0.8" strokeLinecap="round" opacity="0.6" />
    </svg>
  );
}

/** The cable's RJ45 plug head, drawn inserted into the socket and pointing outward. Colour = the link's protocol. */
export function PlugHead({ side, type, count }: { side: "input" | "output"; type: string; count: number }) {
  const fill = cableColor(type);
  const edge = cableOutline(type);
  return (
    <>
      <svg className="apl-plug" data-side={side} data-testid="plug-head" data-type={type} viewBox="0 0 16 10" aria-hidden="true" focusable="false">
        <path d="M0.4 3.2Q0.4 2.4 1.2 2.4H4.2V7.6H1.2Q0.4 7.6 0.4 6.8Z" fill={edge} />
        <rect x="4" y="1.4" width="11" height="7.2" rx="1.1" fill={fill} stroke={edge} strokeWidth="0.8" />
        <rect x="7" y="0.2" width="5" height="1.5" rx="0.4" fill={edge} />
        <path d="M11.2 3.6V6.4M12.6 3.6V6.4M14 3.6V6.4" stroke="oklch(92% 0.08 90)" strokeWidth="0.7" strokeLinecap="round" />
      </svg>
      {count > 1 && <span className="apl-plug-count" data-side={side} data-testid="plug-count" aria-label={`${count} cables`}>×{count}</span>}
    </>
  );
}
