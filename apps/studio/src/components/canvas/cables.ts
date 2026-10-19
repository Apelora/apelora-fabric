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
  kafka: "oklch(48% 0.04 260)",
  pulsar: "oklch(58% 0.12 235)",
  mqtt: "oklch(62% 0.14 320)",
  nats: "oklch(62% 0.12 170)",
  mongodb: "oklch(58% 0.14 145)",
  cassandra: "oklch(60% 0.11 215)",
  memcached: "oklch(62% 0.12 125)",
  grpc: "oklch(60% 0.12 200)",
  ldap: "oklch(60% 0.10 40)",
  smtp: "oklch(64% 0.12 80)",
  syslog: "oklch(55% 0.06 100)",
  nfs: "oklch(58% 0.10 70)",
  mssql: "oklch(52% 0.13 25)",
  smb: "oklch(60% 0.09 250)",
  rdp: "oklch(58% 0.12 225)",
};
export const cableColor = (type: string): string => COLORS[type] ?? COLORS.tcp!;
/** Darker edge for a cable of the given colour. */
export const cableOutline = (color: string): string => `color-mix(in oklab, ${color} 55%, black)`;
/** Cable ends this many px beyond the jack edge, under the plug's strain relief. */
export const CABLE_INSET = 6;
