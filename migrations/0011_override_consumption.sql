-- CLAUDE.md §1 item 6: an override is a full, single-use record — who granted
-- it, which rule it lifts, why, when it expires, and which enrollment
-- transaction consumed it — never a hidden boolean the service consults and
-- forgets. These columns complete the record; the service stamps them in the
-- same transaction as the enrollment change the override authorized.

ALTER TABLE registration_override
    ADD COLUMN consumed_at timestamptz,
    ADD COLUMN consumed_by_enrollment_id uuid REFERENCES enrollment(id);

-- Consumption is atomic: both facts or neither.
ALTER TABLE registration_override
    ADD CONSTRAINT registration_override_consumption_complete
    CHECK ((consumed_at IS NULL) = (consumed_by_enrollment_id IS NULL));

-- The claim query's shape: usable overrides for one student/term/rule.
CREATE INDEX registration_override_claim_lookup
    ON registration_override (student_id, term_id, override_type)
    WHERE consumed_at IS NULL;
