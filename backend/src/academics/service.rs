//! Academic structure commands and reads: terms, courses, sections (whose
//! capacity row is created in the same transaction — CLAUDE.md §1 item 4),
//! meetings, prerequisites, and the catalog queries the student UI reads.

use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::academics::policy::require_can_manage_academics;
use crate::audit::AuditWriter;
use crate::shared::{actor::Actor, error::AppError};

/// Row cap for the registrar's one-page section scanning table
/// (`term_sections_overview`). A full result means truncation, and the
/// page must say so.
pub(crate) const SECTIONS_SCAN_CAP: i64 = 500;

#[derive(Clone)]
pub struct AcademicsService {
    pool: PgPool,
    audit: AuditWriter,
}

#[derive(Debug, Deserialize)]
pub struct CreateTermCommand {
    pub code: String,
    pub name: String,
    pub starts_on: NaiveDate,
    pub ends_on: NaiveDate,
    pub registration_opens_at: DateTime<Utc>,
    pub add_drop_closes_at: DateTime<Utc>,
    pub grade_entry_closes_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
pub struct CreateCourseCommand {
    pub code: String,
    pub title: String,
    pub credit_hours: f64,
}

#[derive(Debug, Deserialize)]
pub struct CreateSectionCommand {
    pub term_id: Uuid,
    pub course_id: Uuid,
    pub section_code: String,
    pub capacity: i32,
}

#[derive(Debug, Deserialize)]
pub struct AddMeetingCommand {
    pub day_of_week: i16,
    pub starts_at: NaiveTime,
    pub ends_at: NaiveTime,
    pub room_id: Option<Uuid>,
}

#[derive(Debug, sqlx::FromRow)]
pub struct CourseSummary {
    pub id: Uuid,
    pub code: String,
    pub title: String,
    pub credit_hours: f64,
    pub active: bool,
    /// "MATH 1101 (≥2), CMPS 1101 (≥2.5)" — aggregated for the courses table.
    pub prerequisites: String,
}

#[derive(Debug, sqlx::FromRow)]
pub struct InstructorOption {
    pub user_id: Uuid,
    pub username: String,
}

#[derive(Debug, sqlx::FromRow)]
pub struct SectionAdminCore {
    pub section_id: Uuid,
    pub course_code: String,
    pub course_title: String,
    pub section_code: String,
    pub status: String,
    pub term_name: String,
    pub capacity: i32,
    pub enrolled_count: i32,
}

#[derive(Debug, sqlx::FromRow)]
pub struct MeetingRow {
    pub day_of_week: i16,
    pub starts_at: NaiveTime,
    pub ends_at: NaiveTime,
}

#[derive(Debug)]
pub struct SectionAdminView {
    pub core: SectionAdminCore,
    pub meetings: Vec<MeetingRow>,
    pub instructors: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateWindowsCommand {
    pub registration_opens_at: DateTime<Utc>,
    pub add_drop_closes_at: DateTime<Utc>,
    pub grade_entry_closes_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
pub struct AddPrerequisiteCommand {
    pub prerequisite_course_id: Uuid,
    pub minimum_grade_points: f64,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct TermSummary {
    pub id: Uuid,
    pub code: String,
    pub name: String,
    pub starts_on: NaiveDate,
    pub ends_on: NaiveDate,
    pub registration_opens_at: DateTime<Utc>,
    pub add_drop_closes_at: DateTime<Utc>,
    pub grade_entry_closes_at: Option<DateTime<Utc>>,
}

/// One row of the registrar's scanning table: a section with its course,
/// seats, meeting summary, and assigned instructors — the whole term from
/// one query (no N+1).
#[derive(Debug, sqlx::FromRow)]
pub struct SectionOverviewRow {
    pub section_id: Uuid,
    pub course_code: String,
    pub course_title: String,
    pub section_code: String,
    pub status: String,
    pub meetings: String,
    /// Comma-joined instructor usernames; empty means unassigned — the
    /// overview flags that, it never hides it.
    pub instructors: String,
    pub capacity: i32,
    pub enrolled_count: i32,
}

/// One catalog row: a section with its course, seats, and meeting summary —
/// everything the browse page shows, from one query (no N+1).
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct CatalogSection {
    pub section_id: Uuid,
    pub course_code: String,
    pub course_title: String,
    pub credit_hours: f64,
    pub section_code: String,
    pub capacity: i32,
    pub enrolled_count: i32,
    /// e.g. "Mon 09:00-10:15 · Rm 214 (Belmopan)", empty when meetings
    /// are not yet scheduled.
    pub meetings: String,
    /// Comma-joined instructor usernames; empty = not yet assigned (TBA).
    pub instructors: String,
    /// Comma-joined prerequisite course codes; empty = none.
    pub prerequisites: String,
    pub description: Option<String>,
    pub faculty: Option<String>,
    /// The viewing student's active enrollment in this section, if any — the
    /// registration screen renders that row in its "enrolled" state and
    /// offers Drop. `None` for a non-student actor or an unenrolled section.
    pub enrolled_enrollment_id: Option<Uuid>,
}

impl AcademicsService {
    pub fn new(pool: PgPool, audit: AuditWriter) -> Self {
        Self { pool, audit }
    }

    pub async fn create_term(
        &self,
        actor: &Actor,
        command: CreateTermCommand,
    ) -> Result<Uuid, AppError> {
        require_can_manage_academics(actor)?;
        let code = required_trimmed(&command.code, "term code")?;
        let name = required_trimmed(&command.name, "term name")?;
        if command.starts_on > command.ends_on {
            return Err(AppError::Validation(
                "term must start before it ends".into(),
            ));
        }
        if command.registration_opens_at >= command.add_drop_closes_at {
            return Err(AppError::Validation(
                "registration must open before add/drop closes".into(),
            ));
        }

        let term_id = Uuid::new_v4();
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            r#"
            INSERT INTO academic_term (
                id, institution_id, code, name, starts_on, ends_on,
                registration_opens_at, add_drop_closes_at, grade_entry_closes_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            "#,
        )
        .bind(term_id)
        .bind(actor.institution_id)
        .bind(code)
        .bind(name)
        .bind(command.starts_on)
        .bind(command.ends_on)
        .bind(command.registration_opens_at)
        .bind(command.add_drop_closes_at)
        .bind(command.grade_entry_closes_at)
        .execute(&mut *tx)
        .await
        .map_err(|e| conflict_on_unique(e, "a term with this code already exists"))?;

        self.audit
            .write(
                &mut tx,
                actor.institution_id,
                actor.user_id,
                "academics.term_created",
                "academic_term",
                term_id,
                &serde_json::json!({ "code": code }),
            )
            .await?;
        tx.commit().await?;
        Ok(term_id)
    }

    pub async fn create_course(
        &self,
        actor: &Actor,
        command: CreateCourseCommand,
    ) -> Result<Uuid, AppError> {
        require_can_manage_academics(actor)?;
        let code = required_trimmed(&command.code, "course code")?;
        let title = required_trimmed(&command.title, "course title")?;
        if !(command.credit_hours > 0.0 && command.credit_hours <= 99.0) {
            return Err(AppError::Validation(
                "credit hours must be between 0 and 99".into(),
            ));
        }

        let course_id = Uuid::new_v4();
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO course (id, institution_id, code, title, credit_hours) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(course_id)
        .bind(actor.institution_id)
        .bind(code)
        .bind(title)
        .bind(command.credit_hours)
        .execute(&mut *tx)
        .await
        .map_err(|e| conflict_on_unique(e, "a course with this code already exists"))?;

        self.audit
            .write(
                &mut tx,
                actor.institution_id,
                actor.user_id,
                "academics.course_created",
                "course",
                course_id,
                &serde_json::json!({ "code": code }),
            )
            .await?;
        tx.commit().await?;
        Ok(course_id)
    }

    /// Creates the section AND its capacity in one transaction. The 0010
    /// trigger inserts the row at capacity 0; setting the real capacity here
    /// means no moment exists where the section is registrable without an
    /// explicit seat count.
    pub async fn create_section(
        &self,
        actor: &Actor,
        command: CreateSectionCommand,
    ) -> Result<Uuid, AppError> {
        require_can_manage_academics(actor)?;
        let section_code = required_trimmed(&command.section_code, "section code")?;
        if command.capacity < 0 {
            return Err(AppError::Validation("capacity cannot be negative".into()));
        }

        let mut tx = self.pool.begin().await?;
        // Both parents must live in the actor's institution; anything else 404s.
        let parents_ok: bool = sqlx::query_scalar(
            r#"
            SELECT EXISTS (SELECT 1 FROM academic_term WHERE id = $1 AND institution_id = $3)
               AND EXISTS (SELECT 1 FROM course WHERE id = $2 AND institution_id = $3)
            "#,
        )
        .bind(command.term_id)
        .bind(command.course_id)
        .bind(actor.institution_id)
        .fetch_one(&mut *tx)
        .await?;
        if !parents_ok {
            return Err(AppError::NotFound);
        }

        let section_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO section (id, institution_id, term_id, course_id, section_code, status) \
             VALUES ($1, $2, $3, $4, $5, 'open')",
        )
        .bind(section_id)
        .bind(actor.institution_id)
        .bind(command.term_id)
        .bind(command.course_id)
        .bind(section_code)
        .execute(&mut *tx)
        .await
        .map_err(|e| conflict_on_unique(e, "this section already exists for the course"))?;

        sqlx::query("UPDATE section_capacity SET capacity = $2 WHERE section_id = $1")
            .bind(section_id)
            .bind(command.capacity)
            .execute(&mut *tx)
            .await?;

        self.audit
            .write(
                &mut tx,
                actor.institution_id,
                actor.user_id,
                "academics.section_created",
                "section",
                section_id,
                &serde_json::json!({
                    "term_id": command.term_id,
                    "course_id": command.course_id,
                    "section_code": section_code,
                    "capacity": command.capacity,
                }),
            )
            .await?;
        tx.commit().await?;
        Ok(section_id)
    }

    /// Registrar seat adjustment. The database CHECK
    /// (`enrolled_count <= capacity`) refuses shrinking below current
    /// enrollment; we pre-check for a friendly message.
    pub async fn set_section_capacity(
        &self,
        actor: &Actor,
        section_id: Uuid,
        capacity: i32,
    ) -> Result<(), AppError> {
        require_can_manage_academics(actor)?;
        if capacity < 0 {
            return Err(AppError::Validation("capacity cannot be negative".into()));
        }

        let mut tx = self.pool.begin().await?;
        let updated = sqlx::query(
            r#"
            UPDATE section_capacity c
               SET capacity = $2, version = c.version + 1
              FROM section s
             WHERE s.id = c.section_id
               AND c.section_id = $1
               AND s.institution_id = $3
               AND c.enrolled_count <= $2
            "#,
        )
        .bind(section_id)
        .bind(capacity)
        .bind(actor.institution_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if updated != 1 {
            // Distinguish "not yours/not there" from "would strand students".
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM section WHERE id = $1 AND institution_id = $2)",
            )
            .bind(section_id)
            .bind(actor.institution_id)
            .fetch_one(&mut *tx)
            .await?;
            return Err(if exists {
                AppError::Conflict("capacity cannot drop below current enrollment".into())
            } else {
                AppError::NotFound
            });
        }

        self.audit
            .write(
                &mut tx,
                actor.institution_id,
                actor.user_id,
                "academics.capacity_set",
                "section",
                section_id,
                &serde_json::json!({ "capacity": capacity }),
            )
            .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn add_meeting(
        &self,
        actor: &Actor,
        section_id: Uuid,
        command: AddMeetingCommand,
    ) -> Result<Uuid, AppError> {
        require_can_manage_academics(actor)?;
        if !(1..=7).contains(&command.day_of_week) {
            return Err(AppError::Validation(
                "day_of_week must be 1 (Monday) through 7 (Sunday)".into(),
            ));
        }
        if command.starts_at >= command.ends_at {
            return Err(AppError::Validation(
                "meeting must start before it ends".into(),
            ));
        }

        let mut tx = self.pool.begin().await?;
        let section_ok: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM section WHERE id = $1 AND institution_id = $2)",
        )
        .bind(section_id)
        .bind(actor.institution_id)
        .fetch_one(&mut *tx)
        .await?;
        if !section_ok {
            return Err(AppError::NotFound);
        }
        if let Some(room_id) = command.room_id {
            let room_ok: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM room WHERE id = $1 AND institution_id = $2)",
            )
            .bind(room_id)
            .bind(actor.institution_id)
            .fetch_one(&mut *tx)
            .await?;
            if !room_ok {
                return Err(AppError::NotFound);
            }
        }

        let meeting_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO section_meeting (id, section_id, day_of_week, starts_at, ends_at, \
             room_id) VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(meeting_id)
        .bind(section_id)
        .bind(command.day_of_week)
        .bind(command.starts_at)
        .bind(command.ends_at)
        .bind(command.room_id)
        .execute(&mut *tx)
        .await?;

        self.audit
            .write(
                &mut tx,
                actor.institution_id,
                actor.user_id,
                "academics.meeting_added",
                "section_meeting",
                meeting_id,
                &serde_json::json!({
                    "section_id": section_id,
                    "day_of_week": command.day_of_week,
                }),
            )
            .await?;
        tx.commit().await?;
        Ok(meeting_id)
    }

    pub async fn add_prerequisite(
        &self,
        actor: &Actor,
        course_id: Uuid,
        command: AddPrerequisiteCommand,
    ) -> Result<(), AppError> {
        require_can_manage_academics(actor)?;
        if course_id == command.prerequisite_course_id {
            return Err(AppError::Validation(
                "a course cannot be its own prerequisite".into(),
            ));
        }

        let mut tx = self.pool.begin().await?;
        let both_ok: bool = sqlx::query_scalar(
            r#"
            SELECT count(*) = 2 FROM course
            WHERE id IN ($1, $2) AND institution_id = $3
            "#,
        )
        .bind(course_id)
        .bind(command.prerequisite_course_id)
        .bind(actor.institution_id)
        .fetch_one(&mut *tx)
        .await?;
        if !both_ok {
            return Err(AppError::NotFound);
        }

        sqlx::query(
            "INSERT INTO course_prerequisite (course_id, prerequisite_course_id, \
             minimum_grade_points) VALUES ($1, $2, $3)",
        )
        .bind(course_id)
        .bind(command.prerequisite_course_id)
        .bind(command.minimum_grade_points)
        .execute(&mut *tx)
        .await
        .map_err(|e| conflict_on_unique(e, "this prerequisite already exists"))?;

        self.audit
            .write(
                &mut tx,
                actor.institution_id,
                actor.user_id,
                "academics.prerequisite_added",
                "course",
                course_id,
                &serde_json::json!({
                    "prerequisite_course_id": command.prerequisite_course_id,
                    "minimum_grade_points": command.minimum_grade_points,
                }),
            )
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Assign an instructor to a section. Idempotent (re-assigning writes no
    /// second audit row). The target must live in the institution AND hold
    /// the instructor role — grading power follows the role system, never a
    /// bare user id.
    pub async fn assign_instructor(
        &self,
        actor: &Actor,
        section_id: Uuid,
        instructor_user_id: Uuid,
    ) -> Result<(), AppError> {
        require_can_manage_academics(actor)?;

        let mut tx = self.pool.begin().await?;
        let section_ok: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM section WHERE id = $1 AND institution_id = $2)",
        )
        .bind(section_id)
        .bind(actor.institution_id)
        .fetch_one(&mut *tx)
        .await?;
        if !section_ok {
            return Err(AppError::NotFound);
        }

        let target_is_instructor: Option<bool> = sqlx::query_scalar(
            r#"
            SELECT EXISTS (
                SELECT 1 FROM user_role ur
                JOIN role r ON r.id = ur.role_id
                WHERE ur.user_id = ua.id AND ur.institution_id = $2 AND r.code = 'instructor'
            )
            FROM user_account ua
            WHERE ua.id = $1 AND ua.institution_id = $2
            "#,
        )
        .bind(instructor_user_id)
        .bind(actor.institution_id)
        .fetch_optional(&mut *tx)
        .await?;
        match target_is_instructor {
            None => return Err(AppError::NotFound),
            Some(false) => {
                return Err(AppError::Validation(
                    "the user does not hold the instructor role".into(),
                ));
            }
            Some(true) => {}
        }

        let inserted = sqlx::query(
            "INSERT INTO instructor_assignment (section_id, instructor_user_id) \
             VALUES ($1, $2) ON CONFLICT DO NOTHING",
        )
        .bind(section_id)
        .bind(instructor_user_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if inserted == 0 {
            return Ok(()); // already assigned
        }

        self.audit
            .write(
                &mut tx,
                actor.institution_id,
                actor.user_id,
                "academics.instructor_assigned",
                "section",
                section_id,
                &serde_json::json!({ "instructor_user_id": instructor_user_id }),
            )
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// All of the institution's terms, newest first — the terms-management
    /// page. Terms accumulate slowly; the cap is a runaway guard.
    pub async fn list_terms(&self, actor: &Actor) -> Result<Vec<TermSummary>, AppError> {
        require_can_manage_academics(actor)?;
        Ok(sqlx::query_as::<_, TermSummary>(
            r#"
            SELECT id, code, name, starts_on, ends_on,
                   registration_opens_at, add_drop_closes_at, grade_entry_closes_at
            FROM academic_term
            WHERE institution_id = $1
            ORDER BY starts_on DESC
            LIMIT 100
            "#,
        )
        .bind(actor.institution_id)
        .fetch_all(&self.pool)
        .await?)
    }

    /// Adjust a term's windows: the ONE shared registration/add/drop window
    /// (ADR: single add-drop deadline) and the grade-entry close. The window
    /// is what the enrollment service enforces, so changing it here changes
    /// what students can actually do — audited in the same transaction.
    pub async fn update_term_windows(
        &self,
        actor: &Actor,
        term_id: Uuid,
        command: UpdateWindowsCommand,
    ) -> Result<(), AppError> {
        require_can_manage_academics(actor)?;
        if command.registration_opens_at >= command.add_drop_closes_at {
            return Err(AppError::Validation(
                "registration must open before add/drop closes".into(),
            ));
        }

        let mut tx = self.pool.begin().await?;
        let updated = sqlx::query(
            r#"
            UPDATE academic_term
               SET registration_opens_at = $3,
                   add_drop_closes_at = $4,
                   grade_entry_closes_at = $5
             WHERE id = $1 AND institution_id = $2
            "#,
        )
        .bind(term_id)
        .bind(actor.institution_id)
        .bind(command.registration_opens_at)
        .bind(command.add_drop_closes_at)
        .bind(command.grade_entry_closes_at)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if updated != 1 {
            return Err(AppError::NotFound);
        }

        self.audit
            .write(
                &mut tx,
                actor.institution_id,
                actor.user_id,
                "academics.term_windows_updated",
                "academic_term",
                term_id,
                &serde_json::json!({
                    "registration_opens_at": command.registration_opens_at,
                    "add_drop_closes_at": command.add_drop_closes_at,
                    "grade_entry_closes_at": command.grade_entry_closes_at,
                }),
            )
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// All courses of the institution for management pages and selects.
    pub async fn list_courses(&self, actor: &Actor) -> Result<Vec<CourseSummary>, AppError> {
        require_can_manage_academics(actor)?;
        Ok(sqlx::query_as::<_, CourseSummary>(
            r#"
            SELECT c.id, c.code, c.title, c.credit_hours::float8 AS credit_hours, c.active,
                   COALESCE(p.summary, '') AS prerequisites
            FROM course c
            LEFT JOIN LATERAL (
                SELECT string_agg(pc.code || ' (≥' || cp.minimum_grade_points || ')',
                                  ', ' ORDER BY pc.code) AS summary
                FROM course_prerequisite cp
                JOIN course pc ON pc.id = cp.prerequisite_course_id
                WHERE cp.course_id = c.id
            ) p ON true
            WHERE c.institution_id = $1
            ORDER BY c.code
            LIMIT 1000
            "#,
        )
        .bind(actor.institution_id)
        .fetch_all(&self.pool)
        .await?)
    }

    /// Users holding the instructor role — the assign-instructor select.
    pub async fn list_instructors(&self, actor: &Actor) -> Result<Vec<InstructorOption>, AppError> {
        require_can_manage_academics(actor)?;
        Ok(sqlx::query_as::<_, InstructorOption>(
            r#"
            SELECT DISTINCT ua.id AS user_id, ua.username
            FROM user_account ua
            JOIN user_role ur ON ur.user_id = ua.id AND ur.institution_id = $1
            JOIN role r ON r.id = ur.role_id AND r.code = 'instructor'
            WHERE ua.institution_id = $1
            ORDER BY ua.username
            LIMIT 1000
            "#,
        )
        .bind(actor.institution_id)
        .fetch_all(&self.pool)
        .await?)
    }

    /// One section for the registrar's detail page: core row, meetings, and
    /// assigned instructors. `None` when the section is not in the actor's
    /// institution (a guessed id 404s).
    pub async fn section_admin(
        &self,
        actor: &Actor,
        section_id: Uuid,
    ) -> Result<Option<SectionAdminView>, AppError> {
        require_can_manage_academics(actor)?;
        let Some(core) = sqlx::query_as::<_, SectionAdminCore>(
            r#"
            SELECT s.id AS section_id, c.code AS course_code, c.title AS course_title,
                   s.section_code, s.status, t.name AS term_name,
                   cap.capacity, cap.enrolled_count
            FROM section s
            JOIN course c ON c.id = s.course_id
            JOIN academic_term t ON t.id = s.term_id
            JOIN section_capacity cap ON cap.section_id = s.id
            WHERE s.id = $1 AND s.institution_id = $2
            "#,
        )
        .bind(section_id)
        .bind(actor.institution_id)
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(None);
        };

        let meetings = sqlx::query_as::<_, MeetingRow>(
            "SELECT day_of_week, starts_at, ends_at FROM section_meeting \
             WHERE section_id = $1 ORDER BY day_of_week, starts_at",
        )
        .bind(section_id)
        .fetch_all(&self.pool)
        .await?;

        let instructors: Vec<String> = sqlx::query_scalar(
            r#"
            SELECT ua.username
            FROM instructor_assignment ia
            JOIN user_account ua ON ua.id = ia.instructor_user_id
            WHERE ia.section_id = $1
            ORDER BY ua.username
            "#,
        )
        .bind(section_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(Some(SectionAdminView {
            core,
            meetings,
            instructors,
        }))
    }

    /// The term whose dates contain today (ties: the latest start).
    pub async fn current_term(&self, actor: &Actor) -> Result<Option<TermSummary>, AppError> {
        let term = sqlx::query_as::<_, TermSummary>(
            r#"
            SELECT id, code, name, starts_on, ends_on,
                   registration_opens_at, add_drop_closes_at, grade_entry_closes_at
            FROM academic_term
            WHERE institution_id = $1
              AND CURRENT_DATE BETWEEN starts_on AND ends_on
            ORDER BY starts_on DESC
            LIMIT 1
            "#,
        )
        .bind(actor.institution_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(term)
    }

    /// Catalog search: open sections of a term, optionally filtered by course
    /// code/title, with seats and an aggregated meeting summary. Paginated —
    /// the section list of a real term is unbounded.
    pub async fn search_catalog(
        &self,
        actor: &Actor,
        term_id: Uuid,
        query: Option<&str>,
        page: u32,
    ) -> Result<Vec<CatalogSection>, AppError> {
        const PAGE_SIZE: i64 = 20;
        let pattern = query
            .map(str::trim)
            .filter(|q| !q.is_empty())
            .map(|q| format!("%{}%", q.replace('%', "\\%").replace('_', "\\_")));

        // Filter/sort/paginate over skinny (code, section_code, id) rows
        // first; the meeting LATERAL, capacity, and enrollment joins then run
        // only for the one page instead of every open section in the term
        // (measured: the per-row LATERAL over the full term dominated the
        // query cost — docs/PERFORMANCE.md, pool/plan audits).
        let sections = sqlx::query_as::<_, CatalogSection>(
            r#"
            SELECT
                s.id AS section_id,
                c.code AS course_code,
                c.title AS course_title,
                c.credit_hours::float8 AS credit_hours,
                c.description,
                c.faculty,
                s.section_code,
                cap.capacity,
                cap.enrolled_count,
                COALESCE(m.summary, '') AS meetings,
                COALESCE(i.names, '') AS instructors,
                COALESCE(p.codes, '') AS prerequisites,
                my_e.id AS enrolled_enrollment_id
            FROM (
                SELECT s.id
                FROM section s
                JOIN course c ON c.id = s.course_id
                WHERE s.institution_id = $1
                  AND s.term_id = $2
                  AND s.status = 'open'
                  AND ($3::text IS NULL OR c.code ILIKE $3 OR c.title ILIKE $3)
                ORDER BY c.code, s.section_code
                LIMIT $4 OFFSET $5
            ) page
            JOIN section s ON s.id = page.id
            JOIN course c ON c.id = s.course_id
            JOIN section_capacity cap ON cap.section_id = s.id
            LEFT JOIN LATERAL (
                SELECT string_agg(
                           (ARRAY['Mon','Tue','Wed','Thu','Fri','Sat','Sun'])[sm.day_of_week]
                           || ' ' || to_char(sm.starts_at, 'HH24:MI')
                           || '-' || to_char(sm.ends_at, 'HH24:MI')
                           || COALESCE(' · Rm ' || r.room_code || ' (' || r.campus_code || ')', ''),
                           ', ' ORDER BY sm.day_of_week, sm.starts_at
                       ) AS summary
                FROM section_meeting sm
                LEFT JOIN room r ON r.id = sm.room_id
                WHERE sm.section_id = s.id
            ) m ON true
            LEFT JOIN LATERAL (
                SELECT string_agg(ua.username, ', ' ORDER BY ua.username) AS names
                FROM instructor_assignment ia
                JOIN user_account ua ON ua.id = ia.instructor_user_id
                WHERE ia.section_id = s.id
            ) i ON true
            LEFT JOIN LATERAL (
                SELECT string_agg(pc.code, ', ' ORDER BY pc.code) AS codes
                FROM course_prerequisite cp
                JOIN course pc ON pc.id = cp.prerequisite_course_id
                WHERE cp.course_id = c.id
            ) p ON true
            LEFT JOIN enrollment my_e
                   ON my_e.section_id = s.id
                  AND my_e.student_id = $6
                  AND my_e.status = 'enrolled'
            ORDER BY c.code, s.section_code
            "#,
        )
        .bind(actor.institution_id)
        .bind(term_id)
        .bind(pattern)
        .bind(PAGE_SIZE)
        .bind(i64::from(page) * PAGE_SIZE)
        .bind(actor.student_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(sections)
    }

    /// One section's catalog row for this student — the true committed state
    /// the registration screen swaps in after a register/drop (seats,
    /// meetings, and whether the student is now enrolled). `None` if the
    /// section is not an open section in the actor's institution.
    pub async fn registration_row(
        &self,
        actor: &Actor,
        section_id: Uuid,
    ) -> Result<Option<CatalogSection>, AppError> {
        Ok(sqlx::query_as::<_, CatalogSection>(
            r#"
            SELECT
                s.id AS section_id,
                c.code AS course_code,
                c.title AS course_title,
                c.credit_hours::float8 AS credit_hours,
                c.description,
                c.faculty,
                s.section_code,
                cap.capacity,
                cap.enrolled_count,
                COALESCE(m.summary, '') AS meetings,
                COALESCE(i.names, '') AS instructors,
                COALESCE(p.codes, '') AS prerequisites,
                my_e.id AS enrolled_enrollment_id
            FROM section s
            JOIN course c ON c.id = s.course_id
            JOIN section_capacity cap ON cap.section_id = s.id
            LEFT JOIN LATERAL (
                SELECT string_agg(
                           (ARRAY['Mon','Tue','Wed','Thu','Fri','Sat','Sun'])[sm.day_of_week]
                           || ' ' || to_char(sm.starts_at, 'HH24:MI')
                           || '-' || to_char(sm.ends_at, 'HH24:MI')
                           || COALESCE(' · Rm ' || r.room_code || ' (' || r.campus_code || ')', ''),
                           ', ' ORDER BY sm.day_of_week, sm.starts_at
                       ) AS summary
                FROM section_meeting sm
                LEFT JOIN room r ON r.id = sm.room_id
                WHERE sm.section_id = s.id
            ) m ON true
            LEFT JOIN LATERAL (
                SELECT string_agg(ua.username, ', ' ORDER BY ua.username) AS names
                FROM instructor_assignment ia
                JOIN user_account ua ON ua.id = ia.instructor_user_id
                WHERE ia.section_id = s.id
            ) i ON true
            LEFT JOIN LATERAL (
                SELECT string_agg(pc.code, ', ' ORDER BY pc.code) AS codes
                FROM course_prerequisite cp
                JOIN course pc ON pc.id = cp.prerequisite_course_id
                WHERE cp.course_id = c.id
            ) p ON true
            LEFT JOIN enrollment my_e
                   ON my_e.section_id = s.id
                  AND my_e.student_id = $3
                  AND my_e.status = 'enrolled'
            WHERE s.id = $1
              AND s.institution_id = $2
              AND s.status = 'open'
            "#,
        )
        .bind(section_id)
        .bind(actor.institution_id)
        .bind(actor.student_id)
        .fetch_optional(&self.pool)
        .await?)
    }

    /// Every section of a term for the registrar's scanning table, with
    /// seats, meetings, and instructors. Optional server-side filter for the
    /// JS-off search floor; the in-page filter handles the rest. Returns at
    /// most [`SECTIONS_SCAN_CAP`] rows — a full result is the caller's
    /// signal that the list was truncated.
    pub async fn term_sections_overview(
        &self,
        actor: &Actor,
        term_id: Uuid,
        query: Option<&str>,
    ) -> Result<Vec<SectionOverviewRow>, AppError> {
        require_can_manage_academics(actor)?;
        // ponytail: one term's sections fit on one scanning page (reference:
        // ~300 rows) and the in-page filter/sort needs the full set loaded;
        // the cap is a runaway guard. It is not silent — the page shows a
        // narrow-your-search notice at the cap (render_sections). Paginate
        // for real if an actual term ever exceeds it.
        const CAP: i64 = SECTIONS_SCAN_CAP;
        let pattern = query
            .map(str::trim)
            .filter(|q| !q.is_empty())
            .map(|q| format!("%{}%", q.replace('%', "\\%").replace('_', "\\_")));

        let rows = sqlx::query_as::<_, SectionOverviewRow>(
            r#"
            SELECT
                s.id AS section_id,
                c.code AS course_code,
                c.title AS course_title,
                s.section_code,
                s.status,
                COALESCE(m.summary, '') AS meetings,
                COALESCE(i.names, '') AS instructors,
                cap.capacity,
                cap.enrolled_count
            FROM section s
            JOIN course c ON c.id = s.course_id
            JOIN section_capacity cap ON cap.section_id = s.id
            LEFT JOIN LATERAL (
                SELECT string_agg(
                           (ARRAY['Mon','Tue','Wed','Thu','Fri','Sat','Sun'])[day_of_week]
                           || ' ' || to_char(starts_at, 'HH24:MI')
                           || '-' || to_char(ends_at, 'HH24:MI'),
                           ', ' ORDER BY day_of_week, starts_at
                       ) AS summary
                FROM section_meeting
                WHERE section_id = s.id
            ) m ON true
            LEFT JOIN LATERAL (
                SELECT string_agg(ua.username, ', ' ORDER BY ua.username) AS names
                FROM instructor_assignment ia
                JOIN user_account ua ON ua.id = ia.instructor_user_id
                WHERE ia.section_id = s.id
            ) i ON true
            WHERE s.institution_id = $1
              AND s.term_id = $2
              AND ($3::text IS NULL
                   OR c.code ILIKE $3 OR c.title ILIKE $3
                   OR COALESCE(i.names, '') ILIKE $3)
            ORDER BY c.code, s.section_code
            LIMIT $4
            "#,
        )
        .bind(actor.institution_id)
        .bind(term_id)
        .bind(pattern)
        .bind(CAP)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }
}

fn required_trimmed<'a>(value: &'a str, field: &str) -> Result<&'a str, AppError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(AppError::Validation(format!("{field} is required")));
    }
    Ok(trimmed)
}

/// Unique-constraint violations are user-visible conflicts (duplicate codes),
/// not internal errors.
fn conflict_on_unique(error: sqlx::Error, message: &str) -> AppError {
    if error
        .as_database_error()
        .and_then(|db| db.code())
        .is_some_and(|code| code == "23505")
    {
        AppError::Conflict(message.into())
    } else {
        AppError::Database(error)
    }
}
