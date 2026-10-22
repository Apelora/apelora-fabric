# ADR 0014 — Fencing by switching a server off through its BMC

Status: **accepted; built (fence provider slice)** · Date: 2026-10-21 · Amends [ADR 0013](0013-fencing-before-restart.md) · Design: [VM recovery on shared storage](../design/vm-recovery-on-shared-storage.md) §3 package 4 · Testing: [without hardware](../guides/testing-without-hardware.md)

## Context

ADR 0013 lets a failed server's modules restart elsewhere only once the server is *fenced*, and offers two proofs: its storage lease is dead (sanlock, about 140 s after its last renewal) or a person confirms it is off. The first is slow, because 140 s is sanlock's floor; the second is slow at night. A server that is cut off from the other servers but still reaches the SAN is neither: its lease stays live, so nothing restarts, which is safe but leaves the application down.

A server's baseboard management controller (BMC) can switch it off whatever state the operating system is in. Doing so is the standard way to fence: once the power state reads back **Off**, the server cannot write to shared storage, whatever it thought it was doing.

The danger is the mirror image of the benefit. A wrong target powers off a healthy server. This is the riskiest code in the project.

## Decision

1. **A third proof, `power-off`.** A member is fenced when Incus reports it Offline **and** its power state, read back from the BMC after a power-off (or already Off), is **Off**. "The power-off request was accepted" is never evidence; only the read-back is. The evidence (provider, system, when) is recorded and audited with the decision, like the other two.
2. **A fence provider in the worker, not in the control plane.** The supervisor leader asks the provider; the control plane never holds BMC credentials. Providers:
   - `redfish`: the standard DMTF Redfish interface over HTTPS (system power state read, `ComputerSystem.Reset` with `ForceOff`). The certificate is verified against the system roots or a CA file named for that BMC; there is no insecure mode. Redfish first.
   - `ipmi` (added 2026-10-22): IPMI 2.0 over the LAN (RMCP+, UDP 623) for BMCs without Redfish. Three requests only: the system GUID, the chassis power state, and a hard power-off. Every session is authenticated, integrity-protected and encrypted: cipher suite 17 (HMAC-SHA256, AES-128) first, suite 3 (HMAC-SHA1, AES-128) only when the BMC refuses 17, and any other answer (suite 0, no integrity, no encryption) is refused. The session asks for the operator role, the least that may switch power, so the BMC user should be an operator, not an administrator. The password is the HMAC key: it is never sent, logged or in an error. The system GUID must equal the mapped UUID (in any of the byte orders BMCs report it) before the power is read or switched. No serial console, virtual media, user management, sensors, boot devices or firmware.
   - `lima`: lab only. Stops a Lima VM with `limactl stop --force`. Enabled only with `APELORA_BMC_ALLOW_LAB=1`, only for VMs named `apelora-…`, never the SAN VM. It exercises the same decision path as a real BMC, because the lab has none.
   - Later: the provisioner's own power control (MAAS, ADR 0002) as another provider.
3. **An allowlist, not discovery.** Providers act only on members named in a mapping file (`APELORA_BMC_MAP`, JSON, no secrets): member → provider, BMC address, user, the name of the secret (`APELORA_BMC_SECRET_…`, read from the worker's environment at the moment of use, never stored, logged or returned), an optional CA file, and **the server's hardware UUID**. The UUID is required for every provider but the lab's: the BMC (or MAAS) must report that exact system UUID before anything is sent, so a mis-cabled or mis-mapped BMC refuses instead of powering off its neighbour.
4. **Guards that fail closed.** Apelora powers a member off only when all of these hold, re-read in the same pass:
   - Incus reports it Offline and has done so for at least `APELORA_BMC_FENCE_AFTER_SECS` (default 30, at least 20);
   - fewer than half of the members are Offline (more points at the control plane's own isolation, not at a failure of those servers);
   - no other member is currently being powered off by Apelora (one at a time; more needs a person);
   - the same member was not powered off in the last ten minutes without being seen Online since (no loop);
   - the worker holds the supervisor leader lease, and every write is fenced by it as before.
5. **Automatic once the allowlist exists.** Configuring the mapping is the operator's approval; nothing else is asked at night. A person can still confirm a server is off (ADR 0013) and that always wins.
6. **Every power-off is audited** (`cluster.member.power_off`, outcome `success`, `failed` or `refused` with the reason) and counted (`apelora_bmc_power_offs_total{outcome}`). Powering off a server that still runs modules is by design what fencing is; the Servers page says what restarts where.
7. **Credentials never leave the worker.** Not in the database, the audit log, logs, the OpenAPI document, the browser or the repository. The effective-settings page lists the mapping's member names and the secret names, never values.

## Consequences

- **What a BMC buys, measured on the lab (2026-10-21, REAL INCUS TESTED, the `lima` provider):** a server cut off from the other servers but still reaching the SAN is fenced at 61 s (Incus Offline at 22 s, the 30 s grace, the power state read back Off); the storage lease alone never fences it (it stays LIVE). **It does not make recovery faster on clustered LVM.** The instance ran again at 220 s, because Incus refuses to start it while sanlock still counts the powered-off host's lease as held, which lasts about 140 s after its last renewal whatever switched it off. The first estimate (40–60 s) was wrong for this storage. Faster restarts need storage that lets a fenced host's locks be taken over at once (not clustered LVM); until then the BMC's value is correctness (no waiting for a person, no running on) not speed.
- A partitioned server that still reaches the SAN is switched off, its modules restart elsewhere, and nobody has to wake up. That is the intended trade.
- The worker needs network access to each BMC on the management network; the control plane does not.
- Real BMCs differ. The Redfish provider is **CONTRACT VERIFIED** against a Redfish-shaped test server until it has run on real hardware. The IPMI provider is **CONTRACT VERIFIED**: its codes and keys against known answers computed separately, a loopback test BMC, and an independent open-source IPMI implementation acting as the BMC on loopback (suite 3 as it ships, suite 17 with its hash switched to SHA-256 for the test; power read, a wrong GUID and a wrong password refused, power-off read back Off). It has not run on a real BMC (`tests/real_ipmi.rs` is the test for one); the lab's REAL INCUS TESTED result covers the decision path, the guards and the recovery that follows, not any vendor's BMC.
- A wrong mapping is the main residual risk; the UUID check and the allowlist narrow it, and the runbook says to verify the mapping with a read-only check before enabling it.

## Alternatives considered

- **IPMI first.** Older and everywhere, but unauthenticated sessions, weak ciphers and vendor quirks; Redfish is the same job over HTTPS with certificates. IPMI was added second, behind the same interface, with only the encrypted suites.
- **IPMI with any cipher suite the BMC offers.** Rejected: suite 0 sends commands without authentication, and the password-hash weakness of RAKP is worse without encryption. A BMC that offers neither 17 nor 3 is refused; use Redfish or MAAS for it.
- **Let Incus's healing do it.** Rejected in ADR 0013: it cannot fence.
- **Ask a person to approve each power-off.** Safe, but no better than today's confirmation path at night. Kept as the fallback, and as the default until a mapping is configured.
- **Discover BMC addresses automatically.** A guess about which controller belongs to which server is exactly the mistake this ADR exists to prevent.
