import { expect, test } from "vitest";

import { LAYER4, describeType, portTypesCompatible } from "./compat";

test("the same type always matches, and only layer-4 types may feed a tcp input", () => {
  expect(portTypesCompatible("http", "http")).toBe(true);
  expect(portTypesCompatible("http", "tcp")).toBe(true);
  expect(portTypesCompatible("tcp", "http")).toBe(false);
  expect(portTypesCompatible("http", "postgres")).toBe(false);
  expect(portTypesCompatible("metrics", "tcp")).toBe(true);
  expect(portTypesCompatible("rdp", "tcp")).toBe(true);
  expect(portTypesCompatible("tcp", "tcp")).toBe(true);
});

test("describeType is the protocol in capitals", () => {
  expect(describeType({ type: "http" })).toBe("HTTP");
  expect(describeType({ type: "postgres" })).toBe("POSTGRES");
});

test("every layer-4 type, and only those names, may feed a tcp input", () => {
  const names = [
    "http", "tcp", "postgres", "mysql", "redis", "amqp", "s3", "metrics", "kafka", "pulsar", "mqtt",
    "nats", "mongodb", "cassandra", "memcached", "grpc", "ldap", "smtp", "syslog", "nfs", "mssql", "smb", "rdp",
  ];
  expect([...LAYER4].sort()).toEqual([...names].sort());
  for (const t of LAYER4) {
    expect(portTypesCompatible(t, "tcp")).toBe(true);
  }
});
