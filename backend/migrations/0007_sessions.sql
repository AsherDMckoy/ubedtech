-- Sessions become opaque-token based: the cookie carries a high-entropy
-- random token and the database stores only its SHA-256. The original
-- schema's bare uuid id as the implied cookie value is not acceptable
-- (predictable, and readable by anyone with database access).
--
-- No application code has ever inserted into user_session (verified in the
-- Phase 0 inspection), so the table is empty everywhere except possibly
-- hand-inserted dev rows — clearing it makes the NOT NULL additions safe.
DELETE FROM user_session;

ALTER TABLE user_session
    ADD COLUMN token_hash bytea NOT NULL,
    -- Sliding idle deadline, refreshed on activity. expires_at remains the
    -- absolute deadline fixed at creation; both must be in the future for a
    -- session to resolve.
    ADD COLUMN idle_expires_at timestamptz NOT NULL;

ALTER TABLE user_session
    ADD CONSTRAINT user_session_token_hash_key UNIQUE (token_hash);

-- Lookups are now by token_hash (served by the unique index above), never by id.
DROP INDEX user_session_active_lookup;

-- The seven product roles. Referenced by code from application logic; ids
-- stay database-local.
INSERT INTO role (code) VALUES
    ('student'),
    ('instructor'),
    ('registrar'),
    ('records_officer'),
    ('document_officer'),
    ('institution_admin'),
    ('platform_licensing_admin')
ON CONFLICT (code) DO NOTHING;
