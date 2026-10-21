-- HA slice B (ADR 0013): a failed member's fencing and the one recovery decision per failure.
-- One open row per member while Incus reports it Offline; closed (resolved_at) once it is Online
-- again. The supervisor leader writes the latest fencing state on every pass, and once: when it
-- was fenced with the evidence, and the recovery decision. A person's confirmation that the
-- server is powered off is recorded on the open row.
CREATE TABLE member_failures (
    id                 uuid PRIMARY KEY,
    member             text NOT NULL,
    detected_at        timestamptz NOT NULL DEFAULT now(),
    -- Incus's last heartbeat from the member, from its message (when readable).
    last_heartbeat     timestamptz,
    -- The latest fencing state (apelora_domain::fencing::FenceView) and when it was written.
    fencing            jsonb,
    fencing_at         timestamptz,
    -- Instances that wait for fencing before they restart elsewhere (alert member-unfenced).
    waiting_instances  integer NOT NULL DEFAULT 0,
    fenced_at          timestamptz,
    fence_evidence     jsonb,
    confirmed_by       text,
    confirmed_at       timestamptz,
    decided_at         timestamptz,
    decision           jsonb,
    resolved_at        timestamptz
);
CREATE UNIQUE INDEX member_failures_open ON member_failures (member) WHERE resolved_at IS NULL;
CREATE INDEX member_failures_detected ON member_failures (detected_at DESC);

-- Each member's sanlock host lease, learned from its own lockspace line while it was Online:
-- the lease Apelora checks once the member fails. A changed generation is never accepted as
-- the same lease.
CREATE TABLE member_lock_hosts (
    member       text PRIMARY KEY,
    lockspace    text NOT NULL,
    host_id      integer NOT NULL,
    generation   bigint NOT NULL,
    observed_at  timestamptz NOT NULL DEFAULT now()
);
