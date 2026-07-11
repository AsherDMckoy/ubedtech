CREATE TYPE enrollment_status AS ENUM ('enrolled', 'dropped', 'withdrawn');

CREATE TABLE student_profile (
    id uuid PRIMARY KEY,
    institution_id uuid NOT NULL REFERENCES institution(id),
    user_id uuid NOT NULL REFERENCES user_account(id),
    student_number text NOT NULL,
    program_code text NOT NULL,
    academic_status text NOT NULL DEFAULT 'good_standing',
    UNIQUE (institution_id, user_id),
    UNIQUE (institution_id, student_number)
);

CREATE TABLE student_term_registration (
    student_id uuid NOT NULL REFERENCES student_profile(id),
    term_id uuid NOT NULL REFERENCES academic_term(id),
    status text NOT NULL DEFAULT 'eligible'
        CHECK (status IN ('eligible', 'blocked', 'closed')),
    hold_flags text[] NOT NULL DEFAULT '{}',
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (student_id, term_id)
);

-- Capacity belongs to enrollment because enrollment owns seat reservation.
CREATE TABLE section_capacity (
    section_id uuid PRIMARY KEY REFERENCES section(id) ON DELETE CASCADE,
    capacity integer NOT NULL CHECK (capacity >= 0),
    enrolled_count integer NOT NULL DEFAULT 0 CHECK (enrolled_count >= 0),
    version bigint NOT NULL DEFAULT 1,
    CHECK (enrolled_count <= capacity)
);

CREATE TABLE enrollment (
    id uuid PRIMARY KEY,
    institution_id uuid NOT NULL REFERENCES institution(id),
    student_id uuid NOT NULL REFERENCES student_profile(id),
    section_id uuid NOT NULL REFERENCES section(id),
    status enrollment_status NOT NULL,
    registered_at timestamptz NOT NULL,
    dropped_at timestamptz,
    source text NOT NULL CHECK (source IN ('student', 'registrar', 'import')),
    idempotency_key uuid NOT NULL,
    created_by_user_id uuid NOT NULL REFERENCES user_account(id),
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (institution_id, student_id, idempotency_key)
);

CREATE UNIQUE INDEX enrollment_one_active_per_section
    ON enrollment (student_id, section_id)
    WHERE status = 'enrolled';

CREATE INDEX enrollment_student_term_read
    ON enrollment (student_id, status, section_id);

CREATE INDEX enrollment_section_active
    ON enrollment (section_id, student_id)
    WHERE status = 'enrolled';

CREATE TABLE registration_override (
    id uuid PRIMARY KEY,
    institution_id uuid NOT NULL REFERENCES institution(id),
    student_id uuid NOT NULL REFERENCES student_profile(id),
    term_id uuid NOT NULL REFERENCES academic_term(id),
    section_id uuid REFERENCES section(id),
    override_type text NOT NULL CHECK (
        override_type IN ('hold', 'prerequisite', 'schedule_conflict', 'capacity', 'deadline')
    ),
    granted_by_user_id uuid NOT NULL REFERENCES user_account(id),
    expires_at timestamptz,
    note text,
    created_at timestamptz NOT NULL DEFAULT now()
);
