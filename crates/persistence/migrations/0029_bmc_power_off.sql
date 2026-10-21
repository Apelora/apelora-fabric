-- Fencing by switching a server off through its BMC (ADR 0014): the open failure keeps when
-- Apelora last switched the member off and how that ended, so a repeat is held back (cooldown)
-- and a second member is not switched off while the first is still down.
ALTER TABLE member_failures
    ADD COLUMN power_off_at     timestamptz,
    ADD COLUMN power_off_result jsonb;
