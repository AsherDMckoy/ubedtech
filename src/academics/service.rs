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
    /// e.g. "1 09:00-10:15, 3 09:00-10:15" (ISO day numbers), empty when
    /// meetings are not yet scheduled.
    pub meetings: String,
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

    /// The term whose dates contain today (ties: the latest start).
    pub async fn current_term(&self, actor: &Actor) -> Result<Option<TermSummary>, AppError> {
        let term = sqlx::query_as::<_, TermSummary>(
            r#"
            SELECT id, code, name, starts_on, ends_on,
                   registration_opens_at, add_drop_closes_at
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

        let sections = sqlx::query_as::<_, CatalogSection>(
            r#"
            SELECT
                s.id AS section_id,
                c.code AS course_code,
                c.title AS course_title,
                c.credit_hours::float8 AS credit_hours,
                s.section_code,
                cap.capacity,
                cap.enrolled_count,
                COALESCE(m.summary, '') AS meetings
            FROM section s
            JOIN course c ON c.id = s.course_id
            JOIN section_capacity cap ON cap.section_id = s.id
            LEFT JOIN LATERAL (
                SELECT string_agg(
                           day_of_week || ' ' || to_char(starts_at, 'HH24:MI')
                           || '-' || to_char(ends_at, 'HH24:MI'),
                           ', ' ORDER BY day_of_week, starts_at
                       ) AS summary
                FROM section_meeting
                WHERE section_id = s.id
            ) m ON true
            WHERE s.institution_id = $1
              AND s.term_id = $2
              AND s.status = 'open'
              AND ($3::text IS NULL OR c.code ILIKE $3 OR c.title ILIKE $3)
            ORDER BY c.code, s.section_code
            LIMIT $4 OFFSET $5
            "#,
        )
        .bind(actor.institution_id)
        .bind(term_id)
        .bind(pattern)
        .bind(PAGE_SIZE)
        .bind(i64::from(page) * PAGE_SIZE)
        .fetch_all(&self.pool)
        .await?;
        Ok(sections)
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
