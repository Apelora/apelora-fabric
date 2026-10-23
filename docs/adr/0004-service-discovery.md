# ADR 0004 — Service discovery: stable addresses, delivered as environment; DNS optional later

Status: **accepted** (2026-10-20, owner's decision: "do what is recommended") · Date: 2026-10-20

## Context

A module must find the modules it is connected to. Phase 07 gives every stateless or
published module a stable *service address* (an OVN load balancer over its replicas), and
every instance is reachable by its instance name on the application's OVN network
(`<instance>.incus`, OVN's built-in DNS). What is missing is a way for the consumer to learn
which address or name to use.

Facts from the test cluster (Incus 7.0.1, OVN 25.03):

- OVN's DNS answers only for instance names it was given by Incus; Apelora cannot add records to
  it through the Incus API.
- Incus network zones are authoritative DNS for *external* resolvers (zone transfers to listed
  peers); they are not served to instances on the network.
- Incus sets `environment.*` instance options at boot and for every exec, for system and OCI
  containers and VMs alike, and they can be changed later (applied at the next boot).

## Decision

1. **The contract is the address, not a name.** A connection from A to B gives A one stable
   target for B: B's service address when B has one, otherwise B's instance name (resolved by
   OVN's DNS). It does not change when replicas come and go.
2. **Delivery through the environment** (`environment.*`, set when the consumer is created):
   `APELORA_SERVICE_<B>` holds that target, where `<B>` is the provider's module id in upper
   case with `-` as `_` (for example `APELORA_SERVICE_WEB_01=198.18.100.1`). Every image and every
   language can read an environment variable; no resolver, sidecar or new infrastructure is
   needed, and nothing changes for modules that ignore it.
3. **Service addresses are allocated when an application is deployed**, before its instances are
   created, so the targets exist when the consumers start. Instances created later (recreated,
   substituted) get the same values.
4. **DNS stays possible without breaking (1)–(3):** an optional resolver module from the
   catalog can serve `<module>.<application>` names inside an application later; the
   environment keeps working alongside it.

## Consequences

- Consumers read one variable per connection; the Studio's connection is the only configuration.
- An instance that already exists keeps the environment it was created with (the plan says kept
  instances keep their network settings); the value only changes if a service address is
  re-allocated, which Apelora does not do while the module remains in the design.
- The variables carry addresses, not secrets; credentials are never put in the environment by
  this mechanism.
- 2026-10-23: step 4 is the catalog class `resolver`. The worker installs `apelora-dns` in that
  instance only, advertises it as the network's nameserver, and has it forward every name it
  does not hold. Applications that do not include it are unchanged. REAL INCUS TESTED the same
  day (`scripts/test-cluster/dns.sh`): a client resolved the service name to the service
  address, and the instance name still resolved.

## Alternatives

- **A resolver module in every application now** — adds an instance (and a failure point) to
  every application before anyone needs names; kept as the later, optional step (4).
- **Records in OVN's DNS** — not available through the Incus API; writing OVN directly would
  bypass Incus (ADR 0001).
- **Incus network zones** — serve external resolvers, not instances.
- **Mounting a generated hosts file** — works only for system containers with a writable
  `/etc/hosts`, not for OCI images; brittle.
