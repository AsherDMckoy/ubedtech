CREATE TABLE academic_term (
    id uuid PRIMARY KEY,
    institution_id uuid NOT NULL REFERENCES institution(id),
    code text NOT NULL,
    name text NOT NULL,
    starts_on date NOT NULL,
    ends_on date NOT NULL,
    registration_opens_at timestamptz NOT NULL,
    registration_closes_at timestamptz NOT NULL,
    drop_add_closes_at timestamptz NOT NULL,
    grade_entry_closes_at timestamptz,
    UNIQUE (institution_id, code),
    CHECK (starts_on <= ends_on),
    CHECK (registration_opens_at < registration_closes_at),
    CHECK (registration_closes_at <= drop_add_closes_at)
);

CREATE TABLE course (
    id uuid PRIMARY KEY,
    institution_id uuid NOT NULL REFERENCES institution(id),
    code text NOT NULL,
    title text NOT NULL,
    credit_hours numeric(4, 1) NOT NULL CHECK (credit_hours > 0),
    active boolean NOT NULL DEFAULT true,
    UNIQUE (institution_id, code)
);

CREATE TABLE course_prerequisite (
    course_id uuid NOT NULL REFERENCES course(id) ON DELETE CASCADE,
    prerequisite_course_id uuid NOT NULL REFERENCES course(id),
    minimum_grade_points double precision NOT NULL DEFAULT 1.0,
    PRIMARY KEY (course_id, prerequisite_course_id),
    CHECK (course_id <> prerequisite_course_id)
);

CREATE TABLE section (
    id uuid PRIMARY KEY,
    institution_id uuid NOT NULL REFERENCES institution(id),
    term_id uuid NOT NULL REFERENCES academic_term(id),
    course_id uuid NOT NULL REFERENCES course(id),
    section_code text NOT NULL,
    status text NOT NULL DEFAULT 'open'
        CHECK (status IN ('draft', 'open', 'closed', 'cancelled')),
    UNIQUE (institution_id, term_id, course_id, section_code)
);

CREATE TABLE room (
    id uuid PRIMARY KEY,
    institution_id uuid NOT NULL REFERENCES institution(id),
    campus_code text NOT NULL,
    room_code text NOT NULL,
    UNIQUE (institution_id, campus_code, room_code)
);

CREATE TABLE section_meeting (
    id uuid PRIMARY KEY,
    section_id uuid NOT NULL REFERENCES section(id) ON DELETE CASCADE,
    day_of_week smallint NOT NULL CHECK (day_of_week BETWEEN 1 AND 7),
    starts_at time NOT NULL,
    ends_at time NOT NULL,
    room_id uuid REFERENCES room(id),
    CHECK (starts_at < ends_at)
);

CREATE INDEX section_meeting_conflict_lookup
    ON section_meeting (section_id, day_of_week, starts_at, ends_at);

CREATE TABLE instructor_assignment (
    section_id uuid NOT NULL REFERENCES section(id) ON DELETE CASCADE,
    instructor_user_id uuid NOT NULL REFERENCES user_account(id),
    assignment_role text NOT NULL DEFAULT 'primary',
    PRIMARY KEY (section_id, instructor_user_id)
);
