CREATE TYPE user_status AS ENUM ('active', 'suspended', 'closed');

CREATE TABLE institution (
    id uuid PRIMARY KEY,
    code text NOT NULL UNIQUE,
    name text NOT NULL,
    timezone text NOT NULL DEFAULT 'America/Belize',
    status text NOT NULL DEFAULT 'active'
        CHECK (status IN ('active', 'inactive')),
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE user_account (
    id uuid PRIMARY KEY,
    institution_id uuid NOT NULL REFERENCES institution(id),
    username text NOT NULL,
    email text NOT NULL,
    status user_status NOT NULL DEFAULT 'active',
    session_version bigint NOT NULL DEFAULT 1,
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (institution_id, username),
    UNIQUE (institution_id, email)
);

CREATE TABLE password_credential (
    user_id uuid PRIMARY KEY REFERENCES user_account(id) ON DELETE CASCADE,
    password_hash text NOT NULL,
    changed_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE user_session (
    id uuid PRIMARY KEY,
    institution_id uuid NOT NULL REFERENCES institution(id),
    user_id uuid NOT NULL REFERENCES user_account(id),
    session_version bigint NOT NULL,
    csrf_secret_hash bytea NOT NULL,
    expires_at timestamptz NOT NULL,
    last_seen_at timestamptz NOT NULL DEFAULT now(),
    revoked_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX user_session_active_lookup
    ON user_session (id, expires_at)
    WHERE revoked_at IS NULL;

CREATE TABLE role (
    id smallserial PRIMARY KEY,
    code text NOT NULL UNIQUE
);

CREATE TABLE user_role (
    id bigserial PRIMARY KEY,
    institution_id uuid NOT NULL REFERENCES institution(id),
    user_id uuid NOT NULL REFERENCES user_account(id) ON DELETE CASCADE,
    role_id smallint NOT NULL REFERENCES role(id),
    scope_type text,
    scope_id uuid,
    UNIQUE NULLS NOT DISTINCT (institution_id, user_id, role_id, scope_type, scope_id)
);

CREATE TABLE audit_event (
    id uuid PRIMARY KEY,
    institution_id uuid NOT NULL REFERENCES institution(id),
    actor_user_id uuid NOT NULL REFERENCES user_account(id),
    action text NOT NULL,
    resource_type text NOT NULL,
    resource_id uuid NOT NULL,
    detail jsonb NOT NULL,
    occurred_at timestamptz NOT NULL
);

CREATE INDEX audit_event_resource_history
    ON audit_event (institution_id, resource_type, resource_id, occurred_at DESC);
