CREATE TABLE transcript_snapshot (
    id uuid PRIMARY KEY,
    institution_id uuid NOT NULL REFERENCES institution(id),
    student_id uuid NOT NULL REFERENCES student_profile(id),
    snapshot_version bigint NOT NULL,
    snapshot_json jsonb NOT NULL,
    content_hash bytea NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (institution_id, student_id, snapshot_version)
);

CREATE TABLE document_request (
    id uuid PRIMARY KEY,
    institution_id uuid NOT NULL REFERENCES institution(id),
    student_id uuid NOT NULL REFERENCES student_profile(id),
    document_type text NOT NULL CHECK (
        document_type IN ('official_transcript', 'enrollment_letter', 'signed_document')
    ),
    status text NOT NULL CHECK (
        status IN ('pending', 'approved', 'rejected', 'generating', 'ready', 'failed')
    ),
    purpose text,
    delivery_method text NOT NULL CHECK (
        delivery_method IN ('download', 'pickup', 'email')
    ),
    current_snapshot_id uuid REFERENCES transcript_snapshot(id),
    requested_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX document_request_admin_queue
    ON document_request (institution_id, status, requested_at);

CREATE INDEX document_request_student_history
    ON document_request (institution_id, student_id, requested_at DESC);

CREATE TABLE document_approval (
    id uuid PRIMARY KEY,
    request_id uuid NOT NULL REFERENCES document_request(id),
    decision text NOT NULL CHECK (decision IN ('approved', 'rejected')),
    decided_by_user_id uuid NOT NULL REFERENCES user_account(id),
    decided_at timestamptz NOT NULL DEFAULT now(),
    note text
);

CREATE TABLE document_job (
    id uuid PRIMARY KEY,
    request_id uuid NOT NULL REFERENCES document_request(id),
    job_type text NOT NULL DEFAULT 'generate_pdf',
    status text NOT NULL CHECK (
        status IN ('queued', 'running', 'complete', 'failed')
    ),
    attempts integer NOT NULL DEFAULT 0,
    available_at timestamptz NOT NULL DEFAULT now(),
    locked_at timestamptz,
    locked_by text,
    last_error text,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX document_job_claim
    ON document_job (status, available_at, created_at)
    WHERE status = 'queued';

CREATE TABLE generated_document (
    id uuid PRIMARY KEY,
    request_id uuid NOT NULL REFERENCES document_request(id),
    snapshot_id uuid REFERENCES transcript_snapshot(id),
    content_hash bytea NOT NULL,
    storage_path text NOT NULL,
    mime_type text NOT NULL,
    size_bytes bigint NOT NULL CHECK (size_bytes >= 0),
    issued_at timestamptz NOT NULL DEFAULT now(),
    superseded_at timestamptz
);

CREATE UNIQUE INDEX generated_document_current
    ON generated_document (request_id)
    WHERE superseded_at IS NULL;
