-- SPDX-License-Identifier: AGPL-3.0-only
-- Checks a restored Apelora database against a backup manifest (psql variable :manifest).
-- Each result row is status|check|detail. Used by db-restore-drill.sh and db-restore-new-host.sh.
-- The append-only probe runs in a transaction that is rolled back, so a kept database is unchanged.

CREATE TEMP TABLE drill_manifest AS SELECT (:'manifest')::jsonb AS j;

SELECT CASE WHEN r.n > 0
             AND r.n = (m.j #>> '{contents,migrations,count}')::bigint
             AND r.latest = (m.j #>> '{contents,migrations,latest}')::bigint
             AND r.fp = m.j #>> '{contents,migrations,fingerprint}'
            THEN 'ok' ELSE 'fail' END,
       'migrations',
       format('restored %s up to version %s; the backup recorded %s up to %s', r.n, r.latest,
              m.j #>> '{contents,migrations,count}', m.j #>> '{contents,migrations,latest}')
FROM drill_manifest m,
     (SELECT count(*) AS n, max(version) AS latest,
             md5(coalesce(string_agg(version::text || ':' || encode(checksum, 'hex'), ',' ORDER BY version), '')) AS fp
      FROM public._sqlx_migrations WHERE success) r;

CREATE TEMP TABLE drill_counts AS
SELECT e.key AS t, e.value::bigint AS recorded,
       CASE WHEN to_regclass(format('public.%I', e.key)) IS NULL THEN NULL
            ELSE (xpath('/row/n/text()', query_to_xml(format('SELECT count(*) AS n FROM public.%I', e.key), false, true, '')))[1]::text::bigint
       END AS restored
FROM drill_manifest m, jsonb_each_text(m.j #> '{contents,row_counts}') e;

SELECT 'fail', 'rows in ' || t,
       CASE WHEN restored IS NULL THEN 'table missing after restore'
            ELSE format('%s rows restored; the backup recorded %s', restored, recorded) END
FROM drill_counts
WHERE restored IS NULL
   OR (recorded > 0 AND restored = 0)
   OR (t = 'audit_events' AND restored < recorded)
ORDER BY t;

SELECT CASE WHEN count(*) > 0 AND bool_and(restored IS NOT NULL) THEN 'ok' ELSE 'fail' END,
       'row counts',
       format('%s tables, %s rows; audit_events %s (backup recorded %s)', count(*), coalesce(sum(restored), 0),
              max(restored) FILTER (WHERE t = 'audit_events'), max(recorded) FILTER (WHERE t = 'audit_events'))
FROM drill_counts;

SELECT CASE WHEN EXISTS (
         SELECT 1 FROM pg_trigger
         WHERE tgname = 'audit_events_no_change' AND tgrelid = to_regclass('public.audit_events')
           AND NOT tgisinternal AND tgenabled IN ('O', 'A'))
       THEN 'ok' ELSE 'fail' END,
       'audit trigger', 'audit_events_no_change is present and enabled';

-- An UPDATE and a DELETE of an audit row must both be refused. With no audit rows, a probe
-- row is added first. The caller rolls this transaction back.
CREATE FUNCTION pg_temp.drill_probe() RETURNS text LANGUAGE plpgsql AS $$
BEGIN
  IF to_regclass('public.audit_events') IS NULL THEN
    RETURN 'fail|audit append-only|audit_events is missing';
  END IF;
  IF NOT EXISTS (SELECT 1 FROM public.audit_events) THEN
    BEGIN
      INSERT INTO public.audit_events (actor, action, target) VALUES ('restore-drill', 'restore-drill.probe', 'scratch database');
    EXCEPTION WHEN others THEN
      RETURN 'fail|audit append-only|no audit rows, and a probe row could not be added to test the trigger';
    END;
  END IF;
  BEGIN
    UPDATE public.audit_events SET actor = actor WHERE id = (SELECT min(id) FROM public.audit_events);
  EXCEPTION WHEN raise_exception THEN
    BEGIN
      DELETE FROM public.audit_events WHERE id = (SELECT min(id) FROM public.audit_events);
    EXCEPTION WHEN raise_exception THEN
      RETURN 'ok|audit append-only|an UPDATE and a DELETE of an audit row were refused';
    END;
    RETURN 'fail|audit append-only|a DELETE of an audit row was allowed';
  END;
  RETURN 'fail|audit append-only|an UPDATE of an audit row was allowed';
END
$$;
BEGIN;
SELECT pg_temp.drill_probe();
ROLLBACK;
