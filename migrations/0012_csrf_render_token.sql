-- Server-rendered pages must embed the session's CSRF token into forms at
-- GET time, and a hash cannot be reversed into the token — so the session
-- row keeps the token itself alongside the hash the middleware compares
-- against (ADR-9). A CSRF token is not an authenticator: without the session
-- cookie (still stored hash-only) it grants nothing, which is why storing it
-- is safe where storing the session token would not be.
--
-- Sessions predating this column resolve to nothing (fail closed, re-login)
-- rather than rendering forms that cannot submit.

ALTER TABLE user_session ADD COLUMN csrf_secret text;
