# ADR 0017 — Apelora follows the newest Incus release

Status: **accepted; built (lock file, lab upgrade script, install guide)** · Date: 2026-10-30 · Amends [ADR 0001](0001-incus-first.md) (which chose the 7.0 long-term series) · Procedure: [upstream Incus](../development/UPSTREAM-INCUS.md)

## Context

ADR 0001 pinned Apelora to Incus 7.0, the long-term series: fixes for two years, security fixes after that, until June 2031. Incus also publishes a feature release about every month. Between 7.0.2 and 7.6.0 those releases fixed things Apelora depends on: cluster evacuation and restoration, OVN load balancers and logical ports, cluster response times, and restricted projects. They also changed behaviour in areas Apelora uses (an evacuation with no target stops instances in place, 7.4; OVN's layer-2 proxy uses proxy ARP, 7.4; OVN logical switch ports persist, 7.3), which only a run on the lab can judge. The owner asked for Apelora to use the newest release.

## Decision

1. **The pin follows the newest 7.x release that has passed the lab tests** (`series: "7"` in `upstream.lock.yaml`). The weekly check proposes each new release in a pull request; the pin moves after the lab run, and `tested_release` and `tested_with` record the evidence.
2. **Hosts install from the `stable` package channel** (`channel: stable`). A major release (8.0) stays a deliberate decision with its own record.
3. **Apelora's code does not require a feature release.** Preflight asks for Incus 7 and the API extensions the adapter uses. The 7.0 long-term series (`lts-7.0`) keeps working and was last tested on 7.0.2 (2026-10-28); later versions of Apelora are tested on the pinned release only.
4. **The lab is upgraded with `scripts/test-cluster/upgrade-incus.sh`** (dry run first; stand-by, voters, database leader last) and the evidence is produced on the pinned release.

## Consequences

- A release is tested about once a month instead of a patch release every few months. The weekly pull request and the lab scripts make that a routine, not a project.
- The `stable` channel offers only its newest release, so a server installed a few days after upstream publishes can be one release ahead of the pin. The Servers page already says "newer than the tested release". The install guide tells operators to hold the packages and to upgrade all servers together.
- Incus upgrades its cluster database one way. The lab cannot return to an older release; testing an older one means rebuilding it.
- A feature release is supported by upstream until the next one. Operators who want the longest support window stay on the long-term series and accept being a release behind what Apelora is tested on.
- Alternatives considered: staying on the long-term series (stable, supported to 2031, but without the fixes above and with a test matrix that no longer matches what the lab runs); testing both series on every change (twice the lab time for a promise nobody asked for).
