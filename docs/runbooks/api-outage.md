# The control plane or the worker is down

**Evidence:** partly verified — the job queue's lease and fencing behaviour is covered by tests
(MOCK VERIFIED). REAL INCUS TESTED (2026-10-20, four members,
`scripts/test-cluster/drill-worker-restart.sh`): a worker killed (`kill -9`) while Incus was
creating an instance; a second worker took the jobs over when the 30 s lease expired, waited on
the operation the first had started, and finished with exactly the three instances asked for; the
same deployment request sent twice queued its jobs once. A longer outage under load is NOT YET
VERIFIED.

- **Running instances are unaffected.** Incus runs them; Apelora is not in the data path. Service
  addresses keep working (they are OVN load balancers).
- **Control plane down:** the Studio and the Fabric Console say the API is not reachable and
  show no invented data. Nothing new can be deployed. Restart it; it needs PostgreSQL
  (`DATABASE_URL`) and its Incus settings.
- **Worker down:** deployments are accepted and queued but not carried out; the replica
  supervisor does not run, so lost replicas are not restored. Restart it; it claims queued jobs
  (a job whose lease expired is taken over, fenced by its generation, and waits for any Incus
  operation an earlier attempt started).
- **PostgreSQL down:** both services fail requests that need the database; Incus keeps running
  everything. Restore the database from backup if needed; job and revision history live there
  ([apelora-database-backup.md](apelora-database-backup.md)).
