-- Course catalog details for the detail modal/expansion: a human
-- description and the faculty the course falls under. Nullable — the UI
-- renders honestly without them ("No description on file yet").
ALTER TABLE course ADD COLUMN description text;
ALTER TABLE course ADD COLUMN faculty text;
