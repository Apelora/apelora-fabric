# A replica fails its health check

Use this page when a replica fails its health check and stays unhealthy.

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
   restarted once (audit: `application.replica.restart-unhealthy`). Only one replica of a
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

## When the module has an application check

A module can also declare a readiness command (Studio → Properties → Application check; [design](../design/application-readiness.md)). The health detail then says which check failed: *port check failing*, *readiness command failing*, or both. A replica whose port answers but whose command fails is a hung application: look inside the instance (`incus exec`, or the terminal in the Console) for what it logs. A command that Incus cannot run (the program is missing from the image) is logged by the worker (`health: the readiness command could not be run`) and decides nothing, so fix the command or the image; the replica is not restarted for it.

## Before you rely on it

- How the supervisor acts on a failing readiness command has not yet been tried with a real failing application. Watch the first one closely.
