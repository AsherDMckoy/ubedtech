-- Phase 8 query-plan inspection: the catalog's per-section meeting summary
-- (LATERAL aggregate) and the student schedule both look up
-- section_meeting by section_id, which had no index — a seq scan per
-- catalog row. Harmless at demo size, quadratic-ish at real scale.

CREATE INDEX section_meeting_by_section
    ON section_meeting (section_id, day_of_week, starts_at);
