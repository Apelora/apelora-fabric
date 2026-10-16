/** Muted cable colours by protocol, so a glance tells what a link carries. */
const COLORS: Record<string, string> = {
  network: "oklch(42% 0.02 260)",
  tcp: "oklch(55% 0.02 260)",
  http: "oklch(58% 0.14 250)",
  postgres: "oklch(52% 0.13 270)",
  mysql: "oklch(58% 0.11 200)",
  redis: "oklch(60% 0.17 25)",
  amqp: "oklch(68% 0.15 60)",
  s3: "oklch(55% 0.14 310)",
  metrics: "oklch(60% 0.13 150)",
};
export const cableColor = (type: string): string => COLORS[type] ?? COLORS.tcp!;
export const cableOutline = (type: string): string => `color-mix(in oklab, ${cableColor(type)} 55%, black)`;
/** Cable ends this many px beyond the jack edge, under the plug's strain relief. */
export const CABLE_INSET = 6;
