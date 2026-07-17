-- Institution events and holidays (plan item 3.4): managed by the
-- institution admin, shown on calendars. Dates are civil dates in the
-- institution's timezone — policy windows stay governed by term dates, so
-- an event here never silently moves a deadline.

CREATE TABLE institution_event (
    id uuid PRIMARY KEY,
    institution_id uuid NOT NULL REFERENCES institution(id),
    title text NOT NULL,
    event_type text NOT NULL CHECK (event_type IN ('holiday', 'event')),
    starts_on date NOT NULL,
    ends_on date NOT NULL,
    created_by_user_id uuid NOT NULL REFERENCES user_account(id),
    created_at timestamptz NOT NULL DEFAULT now(),
    CHECK (starts_on <= ends_on),
    UNIQUE (institution_id, title, starts_on)
);

CREATE INDEX institution_event_by_date
    ON institution_event (institution_id, starts_on);
