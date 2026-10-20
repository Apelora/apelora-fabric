-- Machines being enrolled as cluster members (Phase 09, docs/design/node-enrolment.md). The
-- enrolment state is derived from these facts and from Incus; no join token, BMC credential or
-- other secret is ever stored here.
CREATE TABLE machines (
    name                 text PRIMARY KEY,
    provider             text NOT NULL CHECK (provider IN ('manual', 'maas')),
    -- The provisioner's own identifier (e.g. a MAAS system id); NULL for manual machines.
    provider_ref         text,
    description          text NOT NULL DEFAULT '',
    registered_by        text NOT NULL,
    registered_at        timestamptz NOT NULL DEFAULT now(),
    approved_by          text,
    approved_at          timestamptz,
    -- When the join token issued at approval stops working, as Incus reported it.
    approval_expires_at  text,
    admitted_by          text,
    admitted_at          timestamptz
);
