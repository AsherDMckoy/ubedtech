CREATE TABLE grade_record (
    id uuid PRIMARY KEY,
    institution_id uuid NOT NULL REFERENCES institution(id),
    enrollment_id uuid NOT NULL REFERENCES enrollment(id),
    grade_code text NOT NULL,
    grade_points double precision,
    numeric_value double precision,
    state text NOT NULL CHECK (state IN ('draft', 'published', 'amended')),
    entered_by_user_id uuid NOT NULL REFERENCES user_account(id),
    published_at timestamptz,
    version bigint NOT NULL DEFAULT 1,
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (enrollment_id)
);

CREATE VIEW records_student_course_completion AS
SELECT
    e.student_id,
    s.course_id,
    max(g.grade_points) AS best_grade_points
FROM enrollment e
JOIN section s ON s.id = e.section_id
JOIN grade_record g ON g.enrollment_id = e.id
WHERE g.state IN ('published', 'amended')
  AND g.grade_points IS NOT NULL
GROUP BY e.student_id, s.course_id;
