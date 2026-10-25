import type { Port, PortType } from "./schema";

/** Protocols that may feed a `tcp` input (everything that is itself a layer-4 stream). */
export const LAYER4: ReadonlySet<PortType> = new Set(["http", "tcp", "postgres", "mysql", "redis", "amqp", "s3", "metrics", "kafka", "pulsar", "mqtt", "nats", "mongodb", "cassandra", "memcached", "grpc", "ldap", "smtp", "syslog", "nfs", "mssql", "smb", "rdp", "dns", "bgp", "imap"]);

/** A logical output of type `from` may feed an input of type `to`. */
export function portTypesCompatible(from: PortType, to: PortType): boolean {
  if (from === to) return true;
  return to === "tcp" && LAYER4.has(from);
}

export function describeType(p: Pick<Port, "type">): string {
  return p.type.toUpperCase();
}
