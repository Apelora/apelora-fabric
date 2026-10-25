# The control plane or the worker is down

Use this page when the API does not answer, when deployments stay queued, or when PostgreSQL is down.

- **Running instances are unaffected.** Incus runs them; Apelora is not in the data path. Service
  addresses keep working (they are OVN load balancers).
- **Control plane down:** the Studio and the Fabric Console say the API is not reachable and
  show no invented data. Nothing new can be deployed. Restart it; it needs PostgreSQL
  (`DATABASE_URL`) and its Incus settings.
- **Worker down:** deployments are accepted and queued but not carried out. The replica
  supervisor does not run, so lost replicas are not restored. Restart the worker; it claims queued
  jobs. A job whose 30 s lease expired is taken over, fenced by its generation, and waits for any
  Incus operation an earlier attempt started, so nothing is created twice. A deployment request
  sent twice queues its jobs once.
- **PostgreSQL down:** both services fail requests that need the database; Incus keeps running
  everything. Restore the database from backup if needed: job and revision history live there
  ([apelora-database-backup.md](apelora-database-backup.md)).

## Before you rely on it

- A worker stopped in the middle of a job has been tried; a longer outage under load has not.
