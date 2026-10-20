-- A deployment request repeated with the same Idempotency-Key finds the revision it saved the
-- first time instead of saving the document again.
ALTER TABLE application_revisions ADD COLUMN idempotency_key text UNIQUE;
