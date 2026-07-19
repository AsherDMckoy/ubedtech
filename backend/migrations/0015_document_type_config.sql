-- Per-institution document-type configuration: which document types students
-- may request. The service check fails closed — a missing or disabled row
-- refuses new requests; existing requests are untouched.
--
-- Like section_capacity (0010), the row guarantee lives in the database:
-- every institution gets one row per known type, enabled by default so
-- existing deployments keep their current behavior.

CREATE TABLE institution_document_type (
    institution_id uuid NOT NULL REFERENCES institution(id),
    document_type text NOT NULL CHECK (
        document_type IN ('official_transcript', 'enrollment_letter', 'signed_document')
    ),
    enabled boolean NOT NULL DEFAULT true,
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (institution_id, document_type)
);

INSERT INTO institution_document_type (institution_id, document_type)
SELECT i.id, t.document_type
FROM institution i
CROSS JOIN (
    VALUES ('official_transcript'), ('enrollment_letter'), ('signed_document')
) AS t(document_type)
ON CONFLICT DO NOTHING;

CREATE FUNCTION institution_document_type_guarantee() RETURNS trigger AS $$
BEGIN
    INSERT INTO institution_document_type (institution_id, document_type)
    VALUES
        (NEW.id, 'official_transcript'),
        (NEW.id, 'enrollment_letter'),
        (NEW.id, 'signed_document')
    ON CONFLICT DO NOTHING;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER institution_document_type_guarantee
    AFTER INSERT ON institution
    FOR EACH ROW
    EXECUTE FUNCTION institution_document_type_guarantee();
