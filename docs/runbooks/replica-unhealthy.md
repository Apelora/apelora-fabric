# A replica fails its health check

**Evidence:** REAL INCUS TESTED, 2026-10-20 (Incus 7.0.1, four members) — the end-to-end
demonstration (`scripts/test-cluster/demo.sh`) closes the port of one NGINX replica while it keeps
running and checks each step below.

What Apelora checks: for modules with a service address, the OVN load balancer probes each
replica's service port (a TCP connection, not an application request). Instances without a
service address are not checked; a check that cannot see a hung application behind an open
port will not notice it.

What happens on its own:

1. The load balancer stops sending requests to the replica within seconds of its port closing.
2. After it has failed for `APELORA_HEALTH_FAIL_AFTER_SECS` (default 20 s) the worker records it
   as unhealthy: the Studio overlay shows the module *Degraded* (*Unhealthy* when no replica
   passes), `GET /api/v1/applications/<name>/status` reports `health: unhealthy` with the
   reason.
3. A **stateless** replica unhealthy for `APELORA_HEALTH_RESTART_AFTER_SECS` (default 60 s) is
   restarted once (audit: `application.replica.restart-unhealthy`). Only one replica of an
   module is restarted per pass, and the same replica not again within five minutes, so a
   replica that stays broken is not restarted in a loop. Singleton and stateful modules are
   never restarted automatically.
4. Once it passes for 20 s it is healthy again and receives requests.

What to do when a replica stays unhealthy after its restart:

1. Look at the instance: `incus info <instance> --project <project> --show-log`, and inside it
   (`incus exec`) check that the service listens on its port.
2. Fix the cause (configuration, a dependency it waits for, resources). The replica returns to
   service by itself when its check passes; nothing has to be re-deployed.
3. If the image itself is broken, deploy a corrected design (new image digest): the plan replaces
   the instances.
