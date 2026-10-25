# The deploy identity cannot see its project

Symptoms: Preflight reports *Project `<name>` is not visible to Apelora's Incus identity*;
deployments fail with *User does not have permission for project*; instance lists come back
empty although instances exist.

A common cause: the project was deleted and created again. Incus removes a deleted project from
every restricted certificate's project list. A running server can keep authorising by project name
until it rebuilds its identity cache (for example when a member goes offline), and access then
disappears.

1. On a cluster member, show the restricted certificate:
   `incus config trust show <fingerprint>` — `projects:` lacks the deploy project.
2. Grant it again (keeping its other projects):
   `incus query -X PATCH /1.0/certificates/<fingerprint> --data '{"projects": ["<project>", …]}'`.
3. Check: Preflight passes the project check; `GET /api/v1/instances` lists the project's
   instances again.

Never make the deploy identity unrestricted to work around this (ADR 0003).
