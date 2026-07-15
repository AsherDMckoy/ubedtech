-- CLAUDE.md §1 item 5, resolved: one shared deadline governs both adding and
-- dropping a class. The design docs' split (`registration_closes_at` for adds,
-- `drop_add_closes_at` for drops) left "may a student ADD during the drop/add
-- window?" undecided; the resolution is yes — the whole point of a drop/add
-- window is swapping classes, and a swap needs both actions live at once.
--
-- Consolidation, not a third column: the renamed `add_drop_closes_at` keeps
-- each existing row's drop/add value, so drops behave exactly as before and
-- adds gain the tail of the window they were arbitrarily denied. See
-- ARCHITECTURE_DECISIONS.md ADR-8.

ALTER TABLE academic_term RENAME COLUMN drop_add_closes_at TO add_drop_closes_at;

-- Dropping the column also drops the two CHECK constraints that referenced it
-- (registration_opens_at < registration_closes_at and
-- registration_closes_at <= drop_add_closes_at).
ALTER TABLE academic_term DROP COLUMN registration_closes_at;

ALTER TABLE academic_term
    ADD CONSTRAINT academic_term_window_valid
    CHECK (registration_opens_at < add_drop_closes_at);
