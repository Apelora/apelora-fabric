-- SPDX-License-Identifier: AGPL-3.0-only
-- Deployment workers say they are alive (docs/design/health-and-capacity.md). Each worker
-- writes its row when it starts and every 15 seconds from its own task, and sets stopped_at
-- when it shuts down cleanly. The Fabric Console's Health page reads it; nothing else depends
-- on it. Rows not seen for a day are deleted when a worker starts.
CREATE TABLE worker_heartbeats (
    id          text PRIMARY KEY CHECK (length(id) BETWEEN 1 AND 200),
    host        text NOT NULL,
    version     text NOT NULL,
    started_at  timestamptz NOT NULL DEFAULT now(),
    last_seen   timestamptz NOT NULL DEFAULT now(),
    stopped_at  timestamptz
);
