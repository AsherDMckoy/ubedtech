-- CLAUDE.md §1 item 4: a section without a section_capacity row must not be
-- able to exist. Before this migration a missing capacity row and a full
-- section fell into the same "section is full" path; the seat-reservation
-- UPDATE simply matched zero rows either way.
--
-- The guarantee lives in the database (trigger), not only in the service:
-- any insert path — application, import script, psql — gets a capacity row
-- in the same transaction as the section. New rows start at capacity 0
-- (fail closed: nobody can register until a registrar sets a real capacity).

-- Backfill sections that predate the guarantee. Capacity = the number of
-- active enrollments, so the enrolled_count <= capacity constraint holds and
-- the section is exactly full until a registrar deliberately opens seats.
INSERT INTO section_capacity (section_id, capacity, enrolled_count)
SELECT
    s.id,
    COALESCE(e.active_count, 0),
    COALESCE(e.active_count, 0)
FROM section s
LEFT JOIN (
    SELECT section_id, count(*)::int AS active_count
    FROM enrollment
    WHERE status = 'enrolled'
    GROUP BY section_id
) e ON e.section_id = s.id
ON CONFLICT (section_id) DO NOTHING;

CREATE FUNCTION section_capacity_guarantee() RETURNS trigger AS $$
BEGIN
    INSERT INTO section_capacity (section_id, capacity, enrolled_count)
    VALUES (NEW.id, 0, 0)
    ON CONFLICT (section_id) DO NOTHING;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER section_capacity_guarantee
    AFTER INSERT ON section
    FOR EACH ROW
    EXECUTE FUNCTION section_capacity_guarantee();
