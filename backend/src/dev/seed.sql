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

-- The dev bootstrap term lives strictly in the PAST. current_term picks
-- the term whose dates contain today, and this term has no sections —
-- if it spanned today it would shadow the demo term FALL-2026 below and
-- every student screen would render empty. Load tests reference it by
-- explicit id, so its dates carry no other weight. DO UPDATE repairs
-- databases seeded before this fix.
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
    DATE '2026-05-11',
    DATE '2026-06-26',
    TIMESTAMPTZ '2026-04-27 00:00+00',
    TIMESTAMPTZ '2026-05-25 23:59+00',
    TIMESTAMPTZ '2026-07-10 23:59+00'
)
ON CONFLICT (id) DO UPDATE
SET
    starts_on = EXCLUDED.starts_on,
    ends_on = EXCLUDED.ends_on,
    registration_opens_at = EXCLUDED.registration_opens_at,
    add_drop_closes_at = EXCLUDED.add_drop_closes_at,
    grade_entry_closes_at = EXCLUDED.grade_entry_closes_at;

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

-- ---------------------------------------------------------------------------
-- Demo dataset (frontend sessions): every critical screen has something real
-- to show. Idempotent; layered on the base rows above. Development only.
--
-- Accounts (all passwords: ub-demo-password — never production):
--   demo.student    student, clean record, enrolled in the full section
--   demo.held       student with an ADVISING HOLD (registration blocked)
--   demo.instructor instructor for CMPS-2131 / CMPS-3141 (mixed roster)
--   demo.registrar  registrar + records officer + document officer
--   demo.admin      institution admin (calendar, settings, document types)
--   demo.platform   platform licensing admin (can toggle the UB license
--                   inactive and back from /ui/platform/license)
--
-- Screen fodder in term FALL-2026 (add/drop OPEN now):
--   CMPS-2131 full (4/4) with a roster of published/draft/not-entered grades
--   CMPS-3141 open seats (register against this one)
--   MATH-3201 low seats (3 of 25 left; counter-only, no enrollment rows)
--   PHYS-2101 full AND blocked by unmet prerequisite MATH-2110
--   one PENDING official-transcript request from demo.student
--
-- No waitlist rows: waitlists are not a feature of this backend.
-- ---------------------------------------------------------------------------

BEGIN;

INSERT INTO user_account (id, institution_id, username, email, status) VALUES
    ('00000000-0000-0000-0000-000000000011', '00000000-0000-0000-0000-000000000001', 'demo.student',    'demo.student@example.test',    'active'),
    ('00000000-0000-0000-0000-000000000013', '00000000-0000-0000-0000-000000000001', 'demo.held',       'demo.held@example.test',       'active'),
    ('00000000-0000-0000-0000-000000000015', '00000000-0000-0000-0000-000000000001', 'demo.instructor', 'demo.instructor@example.test', 'active'),
    ('00000000-0000-0000-0000-000000000016', '00000000-0000-0000-0000-000000000001', 'demo.registrar',  'demo.registrar@example.test',  'active'),
    ('00000000-0000-0000-0000-000000000017', '00000000-0000-0000-0000-000000000001', 'demo.admin',      'demo.admin@example.test',      'active'),
    ('00000000-0000-0000-0000-000000000018', '00000000-0000-0000-0000-000000000001', 'demo.platform',   'demo.platform@example.test',   'active'),
    ('00000000-0000-0000-0000-000000000019', '00000000-0000-0000-0000-000000000001', 'demo.roster1',    'demo.roster1@example.test',    'active'),
    ('00000000-0000-0000-0000-00000000001a', '00000000-0000-0000-0000-000000000001', 'demo.roster2',    'demo.roster2@example.test',    'active')
ON CONFLICT (id) DO NOTHING;

-- Argon2id hash of 'ub-demo-password' (same parameters as the app).
INSERT INTO password_credential (user_id, password_hash)
SELECT u.id, '$argon2id$v=19$m=19456,t=2,p=1$99CTbaIfZJFqiVMnv0u5gQ$lqucWyopzupwC/t4+Gf/2F5LI30AyNOoRiOCnWAGOgE'
FROM user_account u
WHERE u.username IN ('demo.student', 'demo.held', 'demo.instructor', 'demo.registrar', 'demo.admin', 'demo.platform')
ON CONFLICT (user_id) DO NOTHING;

INSERT INTO user_role (institution_id, user_id, role_id)
SELECT '00000000-0000-0000-0000-000000000001', u.id, r.id
FROM (VALUES
    ('demo.student',    'student'),
    ('demo.held',       'student'),
    ('demo.roster1',    'student'),
    ('demo.roster2',    'student'),
    ('demo.instructor', 'instructor'),
    ('demo.registrar',  'registrar'),
    ('demo.registrar',  'records_officer'),
    ('demo.registrar',  'document_officer'),
    ('demo.admin',      'institution_admin'),
    ('demo.platform',   'platform_licensing_admin')
) AS grant_list(username, role_code)
JOIN user_account u ON u.username = grant_list.username
JOIN role r ON r.code = grant_list.role_code
ON CONFLICT DO NOTHING;

INSERT INTO student_profile (id, institution_id, user_id, student_number, program_code) VALUES
    ('00000000-0000-0000-0000-000000000012', '00000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-000000000011', 'DEMO-0001', 'CS'),
    ('00000000-0000-0000-0000-000000000014', '00000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-000000000013', 'DEMO-0002', 'CS'),
    ('00000000-0000-0000-0000-00000000001b', '00000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-000000000019', 'DEMO-0003', 'CS'),
    ('00000000-0000-0000-0000-00000000001c', '00000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-00000000001a', 'DEMO-0004', 'CS')
ON CONFLICT (id) DO NOTHING;

-- Fall 2026: registration opened a week ago, add/drop stays open ~5 weeks,
-- grade entry open well past that (the instructor screen needs a live
-- entry window). starts_on must contain today whenever the demo runs
-- before Aug 24 — current_term resolves by date span, and a demo term
-- that hasn't "started" renders every student screen empty. DO UPDATE
-- so re-applying refreshes the windows on an existing database.
INSERT INTO academic_term (id, institution_id, code, name, starts_on, ends_on,
                           registration_opens_at, add_drop_closes_at, grade_entry_closes_at)
VALUES ('00000000-0000-0000-0000-000000000020', '00000000-0000-0000-0000-000000000001',
        'FALL-2026', 'Fall 2026', LEAST(CURRENT_DATE, DATE '2026-08-24'), DATE '2026-12-11',
        now() - interval '7 days', now() + interval '35 days', now() + interval '150 days')
ON CONFLICT (id) DO UPDATE
SET
    starts_on = EXCLUDED.starts_on,
    registration_opens_at = EXCLUDED.registration_opens_at,
    add_drop_closes_at = EXCLUDED.add_drop_closes_at,
    grade_entry_closes_at = EXCLUDED.grade_entry_closes_at;

INSERT INTO course (id, institution_id, code, title, credit_hours) VALUES
    ('00000000-0000-0000-0000-000000000021', '00000000-0000-0000-0000-000000000001', 'CMPS-2131', 'Data structures',      3.0),
    ('00000000-0000-0000-0000-000000000022', '00000000-0000-0000-0000-000000000001', 'CMPS-3141', 'Software engineering', 3.0),
    ('00000000-0000-0000-0000-000000000023', '00000000-0000-0000-0000-000000000001', 'MATH-3201', 'Linear algebra',       3.0),
    ('00000000-0000-0000-0000-000000000024', '00000000-0000-0000-0000-000000000001', 'PHYS-2101', 'Mechanics',            4.0),
    ('00000000-0000-0000-0000-000000000025', '00000000-0000-0000-0000-000000000001', 'MATH-2110', 'Calculus II',          4.0)
ON CONFLICT (id) DO NOTHING;

-- PHYS-2101 requires MATH-2110, which no demo student has completed.
INSERT INTO course_prerequisite (course_id, prerequisite_course_id, minimum_grade_points)
VALUES ('00000000-0000-0000-0000-000000000024', '00000000-0000-0000-0000-000000000025', 1.0)
ON CONFLICT DO NOTHING;

INSERT INTO section (id, institution_id, term_id, course_id, section_code, status) VALUES
    ('00000000-0000-0000-0000-000000000031', '00000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-000000000020', '00000000-0000-0000-0000-000000000021', '01', 'open'),
    ('00000000-0000-0000-0000-000000000032', '00000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-000000000020', '00000000-0000-0000-0000-000000000022', '01', 'open'),
    ('00000000-0000-0000-0000-000000000033', '00000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-000000000020', '00000000-0000-0000-0000-000000000023', '02', 'open'),
    ('00000000-0000-0000-0000-000000000034', '00000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-000000000020', '00000000-0000-0000-0000-000000000024', '01', 'open')
ON CONFLICT (id) DO NOTHING;

INSERT INTO section_meeting (id, section_id, day_of_week, starts_at, ends_at) VALUES
    ('00000000-0000-0000-0000-000000000081', '00000000-0000-0000-0000-000000000031', 1, '09:00', '10:15'),
    ('00000000-0000-0000-0000-000000000082', '00000000-0000-0000-0000-000000000031', 3, '09:00', '10:15'),
    ('00000000-0000-0000-0000-000000000083', '00000000-0000-0000-0000-000000000032', 1, '10:30', '11:45'),
    ('00000000-0000-0000-0000-000000000084', '00000000-0000-0000-0000-000000000032', 3, '10:30', '11:45'),
    ('00000000-0000-0000-0000-000000000085', '00000000-0000-0000-0000-000000000033', 2, '13:00', '14:15'),
    ('00000000-0000-0000-0000-000000000086', '00000000-0000-0000-0000-000000000033', 4, '13:00', '14:15'),
    ('00000000-0000-0000-0000-000000000087', '00000000-0000-0000-0000-000000000034', 2, '08:00', '09:15'),
    ('00000000-0000-0000-0000-000000000088', '00000000-0000-0000-0000-000000000034', 4, '08:00', '09:15')
ON CONFLICT (id) DO NOTHING;

INSERT INTO instructor_assignment (section_id, instructor_user_id) VALUES
    ('00000000-0000-0000-0000-000000000031', '00000000-0000-0000-0000-000000000015'),
    ('00000000-0000-0000-0000-000000000032', '00000000-0000-0000-0000-000000000015')
ON CONFLICT DO NOTHING;

-- Capacities (rows exist via the 0010 trigger; set the demo numbers):
--   CMPS-2131 exactly full with the 4 roster enrollments below.
--   CMPS-3141 wide open. MATH-3201 3 seats left (counter only — the demo
--   needs the low-seats state, not 22 fake students). PHYS-2101 full.
UPDATE section_capacity SET capacity = 4,  enrolled_count = 4  WHERE section_id = '00000000-0000-0000-0000-000000000031';
UPDATE section_capacity SET capacity = 30, enrolled_count = 0  WHERE section_id = '00000000-0000-0000-0000-000000000032';
UPDATE section_capacity SET capacity = 25, enrolled_count = 22 WHERE section_id = '00000000-0000-0000-0000-000000000033';
UPDATE section_capacity SET capacity = 40, enrolled_count = 40 WHERE section_id = '00000000-0000-0000-0000-000000000034';

-- Term eligibility: everyone eligible except demo.held (advising hold).
INSERT INTO student_term_registration (student_id, term_id, status, hold_flags) VALUES
    ('00000000-0000-0000-0000-000000000012', '00000000-0000-0000-0000-000000000020', 'eligible', '{}'),
    ('00000000-0000-0000-0000-000000000014', '00000000-0000-0000-0000-000000000020', 'eligible', '{advising}'),
    ('00000000-0000-0000-0000-00000000001b', '00000000-0000-0000-0000-000000000020', 'eligible', '{}'),
    ('00000000-0000-0000-0000-00000000001c', '00000000-0000-0000-0000-000000000020', 'eligible', '{}')
ON CONFLICT DO NOTHING;

-- The full CMPS-2131 roster (4/4).
INSERT INTO enrollment (id, institution_id, student_id, section_id, status, registered_at, source, idempotency_key, created_by_user_id) VALUES
    ('00000000-0000-0000-0000-000000000051', '00000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-000000000012', '00000000-0000-0000-0000-000000000031', 'enrolled', now() - interval '5 days', 'registrar', '00000000-0000-0000-0000-000000000061', '00000000-0000-0000-0000-000000000016'),
    ('00000000-0000-0000-0000-000000000052', '00000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-000000000014', '00000000-0000-0000-0000-000000000031', 'enrolled', now() - interval '5 days', 'registrar', '00000000-0000-0000-0000-000000000062', '00000000-0000-0000-0000-000000000016'),
    ('00000000-0000-0000-0000-000000000053', '00000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-00000000001b', '00000000-0000-0000-0000-000000000031', 'enrolled', now() - interval '5 days', 'registrar', '00000000-0000-0000-0000-000000000063', '00000000-0000-0000-0000-000000000016'),
    ('00000000-0000-0000-0000-000000000054', '00000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-00000000001c', '00000000-0000-0000-0000-000000000031', 'enrolled', now() - interval '5 days', 'registrar', '00000000-0000-0000-0000-000000000064', '00000000-0000-0000-0000-000000000016')
ON CONFLICT (id) DO NOTHING;

-- Grade mix on that roster: one published, one draft, two not entered.
INSERT INTO grade_record (id, institution_id, enrollment_id, grade_code, grade_points, state, entered_by_user_id, published_at) VALUES
    ('00000000-0000-0000-0000-000000000071', '00000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-000000000053', 'A',  4.0, 'published', '00000000-0000-0000-0000-000000000015', now() - interval '1 day'),
    ('00000000-0000-0000-0000-000000000072', '00000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-000000000054', 'B+', 3.3, 'draft',     '00000000-0000-0000-0000-000000000015', NULL)
ON CONFLICT (id) DO NOTHING;

-- A pending official-document request for the officer queue.
INSERT INTO document_request (id, institution_id, student_id, document_type, status, purpose, delivery_method, idempotency_key)
VALUES ('00000000-0000-0000-0000-000000000075', '00000000-0000-0000-0000-000000000001',
        '00000000-0000-0000-0000-000000000012', 'official_transcript', 'pending',
        'Graduate school application', 'download', '00000000-0000-0000-0000-000000000076')
ON CONFLICT (id) DO NOTHING;

-- ---------------------------------------------------------------------------
-- Academic history: Spring 2025 (fully in the past, every window closed;
-- seed-demo owns the code SPRING-2026, so this term stays out of its way)
-- gives demo.student three published grades, so History/Transcript have
-- real content and the completed-course rule (A37) is demonstrable:
--   CMPS-1121 passed (A)  → offered again in FALL-2026 → register DENIES.
--   MATH-1151 failed (F)  → offered again in FALL-2026 → retake ALLOWED.
--   ENGL-1101 passed (B)  → not offered this term (history fodder only).
-- ---------------------------------------------------------------------------
INSERT INTO academic_term (id, institution_id, code, name, starts_on, ends_on,
                           registration_opens_at, add_drop_closes_at, grade_entry_closes_at)
VALUES ('00000000-0000-0000-0000-000000000090', '00000000-0000-0000-0000-000000000001',
        'SPRING-2025', 'Spring 2025', DATE '2025-01-13', DATE '2025-05-09',
        TIMESTAMPTZ '2024-12-01 00:00:00+00', TIMESTAMPTZ '2025-01-27 00:00:00+00',
        TIMESTAMPTZ '2025-05-23 00:00:00+00')
ON CONFLICT (id) DO NOTHING;

INSERT INTO course (id, institution_id, code, title, credit_hours) VALUES
    ('00000000-0000-0000-0000-000000000091', '00000000-0000-0000-0000-000000000001', 'CMPS-1121', 'Intro to programming', 3.0),
    ('00000000-0000-0000-0000-000000000092', '00000000-0000-0000-0000-000000000001', 'ENGL-1101', 'Composition',          3.0),
    ('00000000-0000-0000-0000-000000000093', '00000000-0000-0000-0000-000000000001', 'MATH-1151', 'College algebra',      3.0)
ON CONFLICT (id) DO NOTHING;

-- Spring 2025 sections (roster history) and the two FALL-2026 re-offerings.
INSERT INTO section (id, institution_id, term_id, course_id, section_code, status) VALUES
    ('00000000-0000-0000-0000-000000000094', '00000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-000000000090', '00000000-0000-0000-0000-000000000091', '01', 'open'),
    ('00000000-0000-0000-0000-000000000095', '00000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-000000000090', '00000000-0000-0000-0000-000000000092', '01', 'open'),
    ('00000000-0000-0000-0000-000000000096', '00000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-000000000090', '00000000-0000-0000-0000-000000000093', '01', 'open'),
    ('00000000-0000-0000-0000-000000000097', '00000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-000000000020', '00000000-0000-0000-0000-000000000091', '01', 'open'),
    ('00000000-0000-0000-0000-000000000098', '00000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-000000000020', '00000000-0000-0000-0000-000000000093', '01', 'open')
ON CONFLICT (id) DO NOTHING;

-- Friday slots so the re-offerings never collide with the existing fodder.
INSERT INTO section_meeting (id, section_id, day_of_week, starts_at, ends_at) VALUES
    ('00000000-0000-0000-0000-000000000099', '00000000-0000-0000-0000-000000000097', 5, '09:00', '10:15'),
    ('00000000-0000-0000-0000-00000000009a', '00000000-0000-0000-0000-000000000098', 5, '10:30', '11:45')
ON CONFLICT (id) DO NOTHING;

UPDATE section_capacity SET capacity = 30, enrolled_count = 0 WHERE section_id = '00000000-0000-0000-0000-000000000097';
UPDATE section_capacity SET capacity = 30, enrolled_count = 0 WHERE section_id = '00000000-0000-0000-0000-000000000098';

-- Spring 2025 seat counters match the one historical enrollment each.
UPDATE section_capacity SET capacity = 25, enrolled_count = 1 WHERE section_id IN
    ('00000000-0000-0000-0000-000000000094',
     '00000000-0000-0000-0000-000000000095',
     '00000000-0000-0000-0000-000000000096');

-- demo.student's Spring 2025 enrollments and published grades.
INSERT INTO enrollment (id, institution_id, student_id, section_id, status, registered_at, source, idempotency_key, created_by_user_id) VALUES
    ('00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-000000000012', '00000000-0000-0000-0000-000000000094', 'enrolled', TIMESTAMPTZ '2025-01-14 14:00:00+00', 'registrar', '00000000-0000-0000-0000-0000000000b1', '00000000-0000-0000-0000-000000000016'),
    ('00000000-0000-0000-0000-0000000000a2', '00000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-000000000012', '00000000-0000-0000-0000-000000000095', 'enrolled', TIMESTAMPTZ '2025-01-14 14:05:00+00', 'registrar', '00000000-0000-0000-0000-0000000000b2', '00000000-0000-0000-0000-000000000016'),
    ('00000000-0000-0000-0000-0000000000a3', '00000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-000000000012', '00000000-0000-0000-0000-000000000096', 'enrolled', TIMESTAMPTZ '2025-01-14 14:10:00+00', 'registrar', '00000000-0000-0000-0000-0000000000b3', '00000000-0000-0000-0000-000000000016')
ON CONFLICT (id) DO NOTHING;

INSERT INTO grade_record (id, institution_id, enrollment_id, grade_code, grade_points, state, entered_by_user_id, published_at) VALUES
    ('00000000-0000-0000-0000-0000000000a4', '00000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-0000000000a1', 'A', 4.0, 'published', '00000000-0000-0000-0000-000000000015', TIMESTAMPTZ '2025-05-16 12:00:00+00'),
    ('00000000-0000-0000-0000-0000000000a5', '00000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-0000000000a2', 'B', 3.0, 'published', '00000000-0000-0000-0000-000000000015', TIMESTAMPTZ '2025-05-16 12:00:00+00'),
    ('00000000-0000-0000-0000-0000000000a6', '00000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-0000000000a3', 'F', 0.0, 'published', '00000000-0000-0000-0000-000000000015', TIMESTAMPTZ '2025-05-16 12:00:00+00')
ON CONFLICT (id) DO NOTHING;

-- ---------------------------------------------------------------------------
-- Current-semester progress grades: demo.student carries two more FALL-2026
-- enrollments whose lecturers grade during the term, so the Grades page
-- shows a live "current standing" mid-semester:
--   CMPS-2515 B+ PUBLISHED  → visible to the student now.
--   STAT-2101 A- DRAFT      → invisible until published (publish it live in
--                             a demo to show the boundary flip).
-- Codes deliberately absent from seed-demo's COURSES list. CMPS-2131 keeps
-- no grade so journey 2's live entry stays untouched.
-- ---------------------------------------------------------------------------
INSERT INTO course (id, institution_id, code, title, credit_hours) VALUES
    ('00000000-0000-0000-0000-0000000000c1', '00000000-0000-0000-0000-000000000001', 'CMPS-2515', 'Web application development', 3.0),
    ('00000000-0000-0000-0000-0000000000c2', '00000000-0000-0000-0000-000000000001', 'STAT-2101', 'Introduction to statistics',  3.0)
ON CONFLICT (id) DO NOTHING;

INSERT INTO section (id, institution_id, term_id, course_id, section_code, status) VALUES
    ('00000000-0000-0000-0000-0000000000c3', '00000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-000000000020', '00000000-0000-0000-0000-0000000000c1', '01', 'open'),
    ('00000000-0000-0000-0000-0000000000c4', '00000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-000000000020', '00000000-0000-0000-0000-0000000000c2', '01', 'open')
ON CONFLICT (id) DO NOTHING;

-- Slots clear of demo.student's CMPS-2131 (Mon/Wed 09:00-10:15).
INSERT INTO section_meeting (id, section_id, day_of_week, starts_at, ends_at) VALUES
    ('00000000-0000-0000-0000-0000000000c5', '00000000-0000-0000-0000-0000000000c3', 2, '15:00', '16:15'),
    ('00000000-0000-0000-0000-0000000000c6', '00000000-0000-0000-0000-0000000000c3', 4, '15:00', '16:15'),
    ('00000000-0000-0000-0000-0000000000c7', '00000000-0000-0000-0000-0000000000c4', 3, '13:00', '14:15')
ON CONFLICT (id) DO NOTHING;

INSERT INTO instructor_assignment (section_id, instructor_user_id) VALUES
    ('00000000-0000-0000-0000-0000000000c3', '00000000-0000-0000-0000-000000000015'),
    ('00000000-0000-0000-0000-0000000000c4', '00000000-0000-0000-0000-000000000015')
ON CONFLICT DO NOTHING;

UPDATE section_capacity SET capacity = 30, enrolled_count = 1 WHERE section_id IN
    ('00000000-0000-0000-0000-0000000000c3',
     '00000000-0000-0000-0000-0000000000c4');

INSERT INTO enrollment (id, institution_id, student_id, section_id, status, registered_at, source, idempotency_key, created_by_user_id) VALUES
    ('00000000-0000-0000-0000-0000000000a7', '00000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-000000000012', '00000000-0000-0000-0000-0000000000c3', 'enrolled', now() - interval '5 days', 'registrar', '00000000-0000-0000-0000-0000000000b4', '00000000-0000-0000-0000-000000000016'),
    ('00000000-0000-0000-0000-0000000000a8', '00000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-000000000012', '00000000-0000-0000-0000-0000000000c4', 'enrolled', now() - interval '5 days', 'registrar', '00000000-0000-0000-0000-0000000000b5', '00000000-0000-0000-0000-000000000016')
ON CONFLICT (id) DO NOTHING;

INSERT INTO grade_record (id, institution_id, enrollment_id, grade_code, grade_points, state, entered_by_user_id, published_at) VALUES
    ('00000000-0000-0000-0000-0000000000a9', '00000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-0000000000a7', 'B+', 3.3, 'published', '00000000-0000-0000-0000-000000000015', now() - interval '2 days'),
    ('00000000-0000-0000-0000-0000000000aa', '00000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-0000000000a8', 'A-', 3.7, 'draft',     '00000000-0000-0000-0000-000000000015', NULL)
ON CONFLICT (id) DO NOTHING;

COMMIT;

-- Course details (0018): description + faculty for the catalog modal.
UPDATE course SET faculty = 'Faculty of Science & Technology',
    description = CASE code
        WHEN 'CMPS-1121' THEN 'First programming course: values, control flow, functions, and small programs, taught in Python. No prior experience assumed.'
        WHEN 'CMPS-2131' THEN 'Arrays, lists, stacks, queues, trees, hash tables, and graphs, with the algorithms that use them and the cost model to reason about both.'
        WHEN 'CMPS-2515' THEN 'Building and deploying server-rendered web applications: HTTP, forms, sessions, databases, and the security failures that follow from getting them wrong.'
        WHEN 'CMPS-3141' THEN 'Team construction of a real system: requirements, design, testing, code review, and delivery, with weekly builds that must keep working.'
        WHEN 'MATH-1151' THEN 'Functions, equations, and inequalities as preparation for calculus; emphasis on fluency over memorization.'
        WHEN 'MATH-2110' THEN 'Integration techniques, sequences and series, and an introduction to differential equations. Continues MATH-1151.'
        WHEN 'MATH-3201' THEN 'Vector spaces, linear maps, matrices, determinants, and eigenvalues, with applications to systems of equations and geometry.'
        WHEN 'PHYS-2101' THEN 'Newtonian mechanics with calculus: kinematics, dynamics, energy, momentum, and rotation, with a weekly laboratory.'
        ELSE description
    END
WHERE code LIKE 'CMPS-%' OR code LIKE 'MATH-%' OR code LIKE 'PHYS-%';

UPDATE course SET faculty = 'Faculty of Education & Arts',
    description = 'Academic writing: argument, structure, evidence, and revision across several graded essays.'
WHERE code = 'ENGL-1101';
