-- Other Incus clusters (sectors) the control plane is told about, besides the one it runs
-- against (ADR 0008). Only what is not secret is kept: the identity's certificate and key and
-- the sector's server certificate are files named by APELORA_SECTOR_<credentials>_* variables.
CREATE TABLE sectors (
    name        text PRIMARY KEY,
    site        text NOT NULL,
    url         text NOT NULL,
    credentials text NOT NULL,
    added_by    text NOT NULL,
    added_at    timestamptz NOT NULL DEFAULT now()
);
