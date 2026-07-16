-- Grade revision history and record immutability, enforced by the database
-- rather than by every service path remembering to write a history row.
--
-- BEFORE UPDATE on grade_record captures the OLD row — prior value, prior
-- state, prior author, prior version — so a correction can never lose what
-- it replaced regardless of which code path performed it. DELETE is refused
-- outright: an academic record is corrected (state 'amended'), never erased.

CREATE TABLE grade_revision (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    grade_record_id uuid NOT NULL REFERENCES grade_record(id),
    grade_code text NOT NULL,
    grade_points double precision,
    numeric_value double precision,
    state text NOT NULL,
    entered_by_user_id uuid NOT NULL,
    version bigint NOT NULL,
    recorded_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX grade_revision_by_record ON grade_revision (grade_record_id, version);

CREATE FUNCTION grade_record_capture_revision() RETURNS trigger AS $$
BEGIN
    INSERT INTO grade_revision (
        grade_record_id, grade_code, grade_points, numeric_value,
        state, entered_by_user_id, version
    )
    VALUES (
        OLD.id, OLD.grade_code, OLD.grade_points, OLD.numeric_value,
        OLD.state, OLD.entered_by_user_id, OLD.version
    );
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER grade_record_capture_revision
    BEFORE UPDATE ON grade_record
    FOR EACH ROW
    EXECUTE FUNCTION grade_record_capture_revision();

CREATE FUNCTION grade_record_no_delete() RETURNS trigger AS $$
BEGIN
    RAISE EXCEPTION 'grade records are never deleted; correct them instead';
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER grade_record_no_delete
    BEFORE DELETE ON grade_record
    FOR EACH ROW
    EXECUTE FUNCTION grade_record_no_delete();

-- A transcript snapshot is an immutable artifact: what was attested once is
-- what stays attested. New facts get a new snapshot version.
CREATE FUNCTION transcript_snapshot_immutable() RETURNS trigger AS $$
BEGIN
    RAISE EXCEPTION 'transcript_snapshot rows are immutable';
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER transcript_snapshot_immutable
    BEFORE UPDATE OR DELETE ON transcript_snapshot
    FOR EACH ROW
    EXECUTE FUNCTION transcript_snapshot_immutable();
