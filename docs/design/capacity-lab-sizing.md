# Capacity: measured on the four-member lab (issue #16)

Status: measured on the disposable test cluster · Date: 2026-10-22 · Script: `scripts/test-cluster/measure.sh`

Lab figures (Lima virtual machines on one workstation), not hardware sizing. Every number is labelled *measured* or *guessed*. Use the Studio's capacity check against what servers report free; these rows say what the control plane, the worker and idle Debian system containers did on this lab.

## Assumptions

| # | Assumption | Value | Source |
|---|---|---|---|
| A1 | Members | 4 × 4 CPU, 8 GiB, aarch64 Lima VMs | given (`incus-vm.yaml`) |
| A2 | Workload | idle Debian 13 system containers, connections on OVN, no service addresses | given (the script) |
| A3 | Control plane and workers | release builds on the workstation, PostgreSQL 18 in Docker | given |
| A4 | Images | already on the members from earlier runs | measured (no first-pull time) |
| A5 | Busy modules, VMs, mixed architectures | not in this run | guessed: higher |

## Runs (measured, 2026-10-22)

| | 40 containers, 1 worker | 120 containers, 4 workers |
|---|---|---|
| Shape | 4 applications × 10 modules | 8 applications × 15 modules |
| Time to all Running | 83 s (2.1 s per instance) | 537 s (4.5 s per instance) |
| Oldest `instance.create` wait | 77 s | 531 s |
| `instance.create` attempts, mean | 40, 1.7 s | 138, 11 s (retries) |
| `network.sync` attempts, mean | 3, 3.9 s | 9, 65 s |
| ACL rule writes, mean | 32, 0.27 s | 79, 5.9 s |
| Supervisor pass (steady 120 s) | 4 passes, 56 ms | 4 passes, 176 ms |
| Incus API so far, mean | 657 calls, 140 ms | 2532 calls, 1.0 s |
| Control plane RSS idle → loaded | 17 → 22 MiB | 17 → 27 MiB |
| Workers RSS idle → loaded | 15 → 18 MiB | 50 → 80 MiB (four processes) |
| PostgreSQL growth | +112 KiB | +168 KiB |
| Placement | 10 per member | 30 per member |
| Members' extra RAM (`free -m`) | +1531 MiB, **38 MiB per container** | +4541 MiB, **37 MiB per container** |
| Incus's figure for one container | ~107 MiB (includes page cache), ~13 MiB disk | same |

The 120-container run with **one** worker did not finish: 115 of 120 Running after 995 s, `PUT /1.0/network-acls/{id}` mean 2.9 s. Four workers got all 120 Running; every `network.sync` still failed its first five attempts (Incus GET timed out at 20 s, or POST returned "already exists" / GET 404 on another member) and succeeded when retried.

Unchanged ACL rules are no longer written: 15 `network.sync` attempts wrote rules 82 times for 120 modules, not 15 × 120.

## Derived

- **Apelora's own processes** are small: tens of mebibytes, and PostgreSQL grew by a fraction of a mebibyte. They are not the sizing problem on this lab.
- **Reconcile:** a supervisor pass at 120 instances is 0.2 s of a 30 s interval. One worker's leader pass covers it. The metric to watch is `apelora_supervisor_pass_duration_seconds`.
- **The bottleneck is Incus writes**, especially ACL rule replacement, once a cluster holds about a hundred objects. Job time is Incus time plus queue wait (one job at a time per worker).
- **Per idle Debian system container on a member:** about **40 MiB RAM** as the kernel counts it, about **15 MiB disk**. Incus's own memory column is larger because it includes page cache. A busy module or a VM is a different number; this run does not provide one.
- **30 idle containers per 8 GiB lab VM** used about 2 GiB extra (~25 % of the VM). That is headroom on this lab, not a promise on hardware.

## Decision

One control plane, a few workers and one PostgreSQL cover this lab with large headroom. Size **members** from what they will run: start from ~40 MiB RAM and ~15 MiB disk per idle Debian system container, add the OS and Incus, and believe the Studio's comparison with free space. Revisit when `network.sync` p95 grows or ACL writes take seconds (`apelora_incus_request_duration_seconds` for `PUT /1.0/network-acls/{id}`, `apelora_worker_job_duration_seconds` for `network.sync`). Hardware, mixed architectures and busy workloads are still **NOT YET VERIFIED**.
