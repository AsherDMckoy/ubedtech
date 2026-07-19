-- Login throttling state, keyed per account + client IP so an attacker
-- hammering one username from one address cannot lock the real user out from
-- everywhere. The window is fixed and self-expiring: once
-- window_started_at is older than the configured window the counter is
-- reset (lazily, on the next failure) — a counter that only ever grows
-- would itself be an account-lockout denial-of-service lever.
CREATE TABLE login_throttle (
    institution_id uuid NOT NULL REFERENCES institution(id),
    username_lower text NOT NULL,
    client_ip text NOT NULL,
    window_started_at timestamptz NOT NULL DEFAULT now(),
    failure_count integer NOT NULL DEFAULT 0,
    PRIMARY KEY (institution_id, username_lower, client_ip)
);
