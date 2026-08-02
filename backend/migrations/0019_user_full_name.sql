-- The persistent header's account menu shows who is signed in. Existing
-- rows default to '' — the UI falls back to the username until a real
-- name is set.
ALTER TABLE user_account
    ADD COLUMN full_name text NOT NULL DEFAULT '';
