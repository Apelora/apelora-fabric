-- Restoring a volume from a backup (docs/architecture/STORAGE.md, docs/runbooks/volumes.md).
-- Before a `volume.restore` job overwrites a volume it copies what is there into the backup pool,
-- as an ordinary backup marked with the job that took it. The mark makes the step idempotent (a
-- retried attempt finds its safety backup instead of taking another) and lets the Console say
-- which backup holds the data from just before a restore. One safety backup per restore job.
ALTER TABLE volume_backups ADD COLUMN before_restore uuid;
CREATE UNIQUE INDEX volume_backups_before_restore ON volume_backups (before_restore)
    WHERE before_restore IS NOT NULL;
