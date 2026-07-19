BEGIN;

-- Fixed development UUIDs make the temporary development Actor predictable.

INSERT INTO institution (
    id,
    code,
    name,
    timezone,
    status
)
VALUES (
    '00000000-0000-0000-0000-000000000001',
    'UB',
    'University of Belize',
    'America/Belize',
    'active'
)
ON CONFLICT (id) DO NOTHING;

INSERT INTO user_account (
    id,
    institution_id,
    username,
    email,
    status
)
VALUES (
    '00000000-0000-0000-0000-000000000002',
    '00000000-0000-0000-0000-000000000001',
    'dev.student',
    'dev.student@example.test',
    'active'
)
ON CONFLICT (id) DO NOTHING;

INSERT INTO student_profile (
    id,
    institution_id,
    user_id,
    student_number,
    program_code,
    academic_status
)
VALUES (
    '00000000-0000-0000-0000-000000000003',
    '00000000-0000-0000-0000-000000000001',
    '00000000-0000-0000-0000-000000000002',
    'DEV-0001',
    'CS',
    'good_standing'
)
ON CONFLICT (id) DO NOTHING;

INSERT INTO academic_term (
    id,
    institution_id,
    code,
    name,
    starts_on,
    ends_on,
    registration_opens_at,
    add_drop_closes_at,
    grade_entry_closes_at
)
VALUES (
    '00000000-0000-0000-0000-000000000005',
    '00000000-0000-0000-0000-000000000001',
    'DEV-2026',
    'Development Term 2026',
    CURRENT_DATE,
    CURRENT_DATE + 120,
    now() - interval '7 days',
    now() + interval '37 days',
    now() + interval '150 days'
)
ON CONFLICT (id) DO NOTHING;

INSERT INTO institution_license (
    institution_id,
    deployment_id,
    mode,
    status,
    valid_from,
    valid_until,
    feature_set,
    version
)
VALUES (
    '00000000-0000-0000-0000-000000000001',
    '00000000-0000-0000-0000-000000000004',
    'hosted',
    'active',
    now() - interval '1 day',
    now() + interval '1 year',
    '{"demo": true}'::jsonb,
    1
)
ON CONFLICT (institution_id) DO UPDATE
SET
    status = 'active',
    valid_until = now() + interval '1 year',
    updated_at = now();

COMMIT;
