import { useEffect, useState } from "react";

/**
 * The sign-in page's illustration: servers on a grid, connected along the grid lines, with traffic
 * travelling between them and now and then a module moving from one server to another.
 * Decorative (hidden from assistive technology); still when the person asks for reduced motion.
 */

type P = readonly [number, number];

const NODES: P[] = [
  [120, 160], [280, 120], [440, 200], [200, 320], [360, 320], [520, 360], [120, 440], [280, 480], [440, 480],
];

// Routes run along the grid lines between servers: [points, seconds per trip, start offset].
const ROUTES: [P[], number, number][] = [
  [[[120, 160], [120, 120], [280, 120]], 2.8, 0],
  [[[280, 120], [440, 120], [440, 200]], 3.2, 0.8],
  [[[440, 200], [440, 320], [360, 320]], 2.6, 1.6],
  [[[200, 320], [360, 320]], 2.2, 0.4],
  [[[120, 160], [120, 320], [200, 320]], 3, 2.2],
  [[[360, 320], [520, 320], [520, 360]], 2.4, 1.1],
  [[[200, 320], [200, 440], [120, 440]], 2.6, 2.8],
  [[[280, 480], [440, 480]], 2.2, 0.2],
  [[[360, 320], [360, 480], [440, 480]], 3.4, 1.9],
  [[[120, 440], [120, 480], [280, 480]], 2.8, 3.1],
];
// The module that moves between two servers (and back).
const MOVE: P[] = [[280, 120], [280, 240], [200, 240], [200, 320]];

// Soft, distinct colours per server; links blend from one server's colour to the other's.
const COLORS = ["#14B8A6", "#6366F1", "#0EA5E9", "#F59E0B", "#F43F5E", "#8B5CF6", "#10B981", "#EC4899", "#3B82F6"];
const colorAt = ([x, y]: P) => COLORS[Math.max(0, NODES.findIndex(([nx, ny]) => nx === x && ny === y)) % COLORS.length]!;

const d = (pts: P[]) => pts.map(([x, y], i) => `${i ? "L" : "M"}${x} ${y}`).join(" ");

function useReducedMotion() {
  const query = "(prefers-reduced-motion: reduce)";
  const [reduce, setReduce] = useState(() => typeof window !== "undefined" && !!window.matchMedia?.(query).matches);
  useEffect(() => {
    const m = window.matchMedia?.(query);
    if (!m) return;
    const on = () => setReduce(m.matches);
    m.addEventListener("change", on);
    return () => m.removeEventListener("change", on);
  }, []);
  return reduce;
}

export function SignInArt() {
  const still = useReducedMotion();
  return (
    <svg viewBox="0 -60 640 720" preserveAspectRatio="xMidYMid slice" className="h-full w-full" aria-hidden="true" focusable="false">
      <defs>
        <pattern id="apl-grid" width="40" height="40" patternUnits="userSpaceOnUse">
          <path d="M40 0H0V40" fill="none" stroke="#64748B" strokeOpacity="0.08" strokeWidth="1" />
        </pattern>
        <radialGradient id="apl-fade" cx="50%" cy="50%" r="55%">
          <stop offset="0.5" stopColor="#fff" />
          <stop offset="1" stopColor="#000" />
        </radialGradient>
        <mask id="apl-mask"><rect y="-60" width="640" height="720" fill="url(#apl-fade)" /></mask>
        <filter id="apl-glow" x="-200%" y="-200%" width="500%" height="500%">
          <feGaussianBlur stdDeviation="2.5" result="b" />
          <feMerge><feMergeNode in="b" /><feMergeNode in="SourceGraphic" /></feMerge>
        </filter>
        {ROUTES.map(([pts], i) => {
          const [a, b] = [pts[0]!, pts[pts.length - 1]!];
          return (
            <linearGradient key={i} id={`apl-route-${i}`} gradientUnits="userSpaceOnUse" x1={a[0]} y1={a[1]} x2={b[0]} y2={b[1]}>
              <stop offset="0" stopColor={colorAt(a)} />
              <stop offset="1" stopColor={colorAt(b)} />
            </linearGradient>
          );
        })}
      </defs>

      <g mask="url(#apl-mask)">
        <rect y="-60" width="640" height="720" fill="url(#apl-grid)" />
        {ROUTES.map(([pts], i) => (
          <path key={d(pts)} d={d(pts)} fill="none" stroke={`url(#apl-route-${i})`} strokeOpacity="0.4" strokeWidth="1.75" strokeLinecap="round" strokeLinejoin="round" />
        ))}

        {!still && ROUTES.map(([pts, dur, begin]) => (
          <circle key={`p-${d(pts)}`} r="3.5" fill={colorAt(pts[0]!)} filter="url(#apl-glow)" opacity="0">
            <animateMotion dur={`${dur}s`} begin={`${begin}s`} repeatCount="indefinite" path={d(pts)} />
            <animate attributeName="opacity" dur={`${dur}s`} begin={`${begin}s`} repeatCount="indefinite" values="0;0.9;0.9;0" keyTimes="0;0.1;0.85;1" />
          </circle>
        ))}

        {NODES.map((n, i) => {
          const c = colorAt(n);
          return (
            <g key={`${n[0]}-${n[1]}`} transform={`translate(${n[0]} ${n[1]})`}>
              {!still && (
                <rect x="-12" y="-12" width="24" height="24" rx="7" fill="none" stroke={c} strokeWidth="1.5" opacity="0">
                  <animate attributeName="opacity" values="0.5;0" dur="2.6s" begin={`${(i * 0.7) % 3}s`} repeatCount="indefinite" />
                  <animateTransform attributeName="transform" type="scale" values="1;1.9" dur="2.6s" begin={`${(i * 0.7) % 3}s`} repeatCount="indefinite" />
                </rect>
              )}
              <rect x="-12" y="-12" width="24" height="24" rx="7" fill="#FFFFFF" fillOpacity="0.85" stroke={c} strokeOpacity="0.45" strokeWidth="1.5" />
              <rect x="-4" y="-4" width="8" height="8" rx="2.5" fill={c} fillOpacity="0.8" />
            </g>
          );
        })}

        {!still && (
          <rect x="-7" y="-7" width="14" height="14" rx="4" fill="#FFFFFF" stroke="#6366F1" strokeWidth="2" filter="url(#apl-glow)">
            <animateMotion dur="9s" repeatCount="indefinite" path={d(MOVE)} keyPoints="0;0;1;1;0" keyTimes="0;0.3;0.5;0.8;1" calcMode="spline" keySplines="0 0 1 1;0.65 0 0.35 1;0 0 1 1;0.65 0 0.35 1" />
          </rect>
        )}
      </g>
    </svg>
  );
}
