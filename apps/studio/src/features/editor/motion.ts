/** Motion tokens (ms). Keep in sync with the CSS custom properties in styles/index.css. */
export const MOTION = { fast: 120, base: 180, slow: 260 } as const;

export const prefersReducedMotion = (): boolean => typeof window !== "undefined" && !!window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;

/** Duration for programmatic viewport animation: 0 when the user prefers reduced motion. */
export const ms = (n: number): number => (prefersReducedMotion() ? 0 : n);
