-- Commercial terms are platform-operator data, not student billing data.
-- Money is stored in minor currency units to avoid floating-point amounts.
CREATE TABLE institution_contract (
    institution_id uuid PRIMARY KEY REFERENCES institution(id),
    contract_reference text NOT NULL UNIQUE,
    billing_model text NOT NULL CHECK (billing_model IN ('annual', 'contractual')),
    deployment_mode text NOT NULL CHECK (deployment_mode IN ('hosted', 'self_hosted')),
    currency_code char(3) NOT NULL,
    software_fee_minor bigint NOT NULL CHECK (software_fee_minor >= 0),
    hosting_fee_minor bigint,
    installation_fee_minor bigint,
    starts_at timestamptz NOT NULL,
    ends_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CHECK (starts_at < ends_at),
    CHECK (
        (deployment_mode = 'hosted'
            AND hosting_fee_minor IS NOT NULL
            AND hosting_fee_minor >= 0
            AND installation_fee_minor IS NULL)
        OR
        (deployment_mode = 'self_hosted'
            AND installation_fee_minor IS NOT NULL
            AND installation_fee_minor >= 0
            AND hosting_fee_minor IS NULL)
    )
);

CREATE TABLE institution_license (
    institution_id uuid PRIMARY KEY REFERENCES institution(id),
    deployment_id uuid NOT NULL,
    mode text NOT NULL CHECK (mode IN ('hosted', 'self_hosted')),
    status text NOT NULL CHECK (status IN ('active', 'suspended', 'expired')),
    valid_from timestamptz NOT NULL,
    valid_until timestamptz NOT NULL,
    feature_set jsonb NOT NULL DEFAULT '{}',
    version bigint NOT NULL DEFAULT 1,
    updated_at timestamptz NOT NULL DEFAULT now(),
    CHECK (valid_from < valid_until)
);

CREATE TABLE license_change (
    id uuid PRIMARY KEY,
    institution_id uuid NOT NULL REFERENCES institution(id),
    old_status text NOT NULL,
    new_status text NOT NULL,
    changed_by_user_id uuid NOT NULL REFERENCES user_account(id),
    reason text NOT NULL,
    changed_at timestamptz NOT NULL DEFAULT now()
);
