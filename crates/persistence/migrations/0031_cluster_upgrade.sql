-- One rolling Incus upgrade at a time (the assistant on Servers → Upgrades).
-- Apelora records the order and which step each member is on. It does not install Incus.
CREATE TABLE cluster_upgrades (
    id          uuid PRIMARY KEY,
    status      text NOT NULL CHECK (status IN ('running', 'done', 'abandoned')),
    started_by  text NOT NULL,
    started_at  timestamptz NOT NULL DEFAULT now(),
    finished_at timestamptz,
    steps       jsonb NOT NULL
);

-- At most one run is in progress.
CREATE UNIQUE INDEX cluster_upgrades_one_running
    ON cluster_upgrades ((true))
    WHERE status = 'running';
