-- Duplicate-submission guarantee for document requests (Phase 7 hardening):
-- the request form carries a server-minted idempotency key, and resubmitting
-- the same form (double-click, retry, replayed POST) returns the original
-- request instead of filing a second one. Same shape as enrollment (0003).

ALTER TABLE document_request ADD COLUMN idempotency_key uuid;

UPDATE document_request
   SET idempotency_key = gen_random_uuid()
 WHERE idempotency_key IS NULL;

ALTER TABLE document_request ALTER COLUMN idempotency_key SET NOT NULL;

ALTER TABLE document_request
    ADD CONSTRAINT document_request_idempotent
    UNIQUE (institution_id, student_id, idempotency_key);
