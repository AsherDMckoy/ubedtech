-- Load-test dataset, applied AFTER src/dev/seed.sql on a dedicated
-- database (load/README.md). Idempotent.
--
-- Adds: a password credential for dev.student (password:
-- load-test-password-123 — load databases are throwaway, never reuse this
-- hash anywhere real), 200 courses × 1 section each with capacity 50 and
-- one meeting, so the catalog read path has realistic joins and paging.

BEGIN;

INSERT INTO password_credential (user_id, password_hash)
VALUES (
    '00000000-0000-0000-0000-000000000002',
    '$argon2id$v=19$m=19456,t=2,p=1$wDwUtovs4+A/wnn240BYjg$qCtI+9pq+8tnxYPyp4+mELysjMiG7pvzTi5nsEIseiY'
)
ON CONFLICT (user_id) DO NOTHING;

INSERT INTO user_role (institution_id, user_id, role_id)
SELECT '00000000-0000-0000-0000-000000000001',
       '00000000-0000-0000-0000-000000000002',
       id
FROM role WHERE code = 'student'
ON CONFLICT DO NOTHING;

-- 200 courses, one open section each. The section trigger creates the
-- capacity row at 0; raise it to 50 afterwards.
INSERT INTO course (id, institution_id, code, title, credit_hours)
SELECT gen_random_uuid(),
       '00000000-0000-0000-0000-000000000001',
       'LOAD-' || n,
       'Load Test Course ' || n,
       3.0
FROM generate_series(1, 200) AS n
ON CONFLICT DO NOTHING;

INSERT INTO section (id, institution_id, term_id, course_id, section_code, status)
SELECT gen_random_uuid(),
       '00000000-0000-0000-0000-000000000001',
       '00000000-0000-0000-0000-000000000005',
       c.id,
       'A',
       'open'
FROM course c
WHERE c.code LIKE 'LOAD-%'
  AND NOT EXISTS (SELECT 1 FROM section s WHERE s.course_id = c.id)
ON CONFLICT DO NOTHING;

UPDATE section_capacity sc
   SET capacity = 50
  FROM section s
  JOIN course c ON c.id = s.course_id
 WHERE sc.section_id = s.id
   AND c.code LIKE 'LOAD-%'
   AND sc.capacity = 0;

INSERT INTO section_meeting (id, section_id, day_of_week, starts_at, ends_at)
SELECT gen_random_uuid(), s.id, 1 + (row_number() OVER ()) % 5,
       '09:00', '10:30'
FROM section s
JOIN course c ON c.id = s.course_id
WHERE c.code LIKE 'LOAD-%'
  AND NOT EXISTS (SELECT 1 FROM section_meeting m WHERE m.section_id = s.id);

COMMIT;
