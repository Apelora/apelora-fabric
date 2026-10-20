-- SPDX-License-Identifier: AGPL-3.0-only
-- The Incus operation a job started, recorded before waiting on it. After a crash or a lost
-- lease the next attempt waits on this operation (or re-reads Incus) instead of repeating the
-- call.
ALTER TABLE jobs ADD COLUMN external_operation text;
