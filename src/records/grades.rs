use crate::audit::AuditWriter;
use crate::shared::{
    actor::{Actor, Role},
    error::AppError,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Clone)]
pub struct GradeService {
    pool: PgPool,
    audit: AuditWriter,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct StudentGradeRow {
    pub course_code: String,
    pub course_title: String,
    pub section_code: String,
    pub grade_code: String,
    pub published_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
pub struct SaveGradeCommand {
    pub enrollment_id: Uuid,
    pub grade_code: String,
    pub grade_points: Option<f64>,
    pub numeric_value: Option<f64>,
    pub expected_version: i64,
}

/// Post-publication correction by the records office: the prior value and
/// author survive in `grade_revision` (database trigger), the new state is
/// `amended`, and the required reason lands in the audit trail.
#[derive(Debug, Deserialize)]
pub struct CorrectGradeCommand {
    pub enrollment_id: Uuid,
    pub grade_code: String,
    pub grade_points: Option<f64>,
    pub numeric_value: Option<f64>,
    pub reason: String,
    pub expected_version: i64,
}

/// One section an instructor teaches, for the "my sections" page.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct InstructorSection {
    pub section_id: Uuid,
    pub course_code: String,
    pub course_title: String,
    pub section_code: String,
    pub term_name: String,
    pub enrolled_count: i64,
}

/// One line of the student's academic history (published/amended only).
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct HistoryRow {
    pub term_code: String,
    pub term_name: String,
    pub course_code: String,
    pub course_title: String,
    pub credit_hours: f64,
    pub grade_code: String,
    pub grade_points: Option<f64>,
    pub state: String,
}

/// One roster line: the enrollment plus its grade state, if any. `state`
/// None means no grade entered yet ("pending" in the UI).
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct RosterRow {
    pub enrollment_id: Uuid,
    pub student_number: String,
    pub student_name: String,
    pub grade_code: Option<String>,
    pub state: Option<String>,
    pub version: Option<i64>,
}

impl GradeService {
    pub fn new(pool: PgPool, audit: AuditWriter) -> Self {
        Self { pool, audit }
    }

    /// The sections this instructor is assigned to — the ONLY sections their
    /// roster and grading powers reach.
    pub async fn instructor_sections(
        &self,
        actor: &Actor,
    ) -> Result<Vec<InstructorSection>, AppError> {
        if !actor.has_role(Role::Instructor) {
            return Err(AppError::Forbidden);
        }
        let sections = sqlx::query_as::<_, InstructorSection>(
            r#"
            SELECT
                s.id AS section_id,
                c.code AS course_code,
                c.title AS course_title,
                s.section_code,
                t.name AS term_name,
                count(e.id) AS enrolled_count
            FROM instructor_assignment ia
            JOIN section s ON s.id = ia.section_id
            JOIN course c ON c.id = s.course_id
            JOIN academic_term t ON t.id = s.term_id
            LEFT JOIN enrollment e ON e.section_id = s.id AND e.status = 'enrolled'
            WHERE ia.instructor_user_id = $1
              AND s.institution_id = $2
            GROUP BY s.id, c.id, t.id
            ORDER BY t.starts_on DESC, c.code, s.section_code
            "#,
        )
        .bind(actor.user_id)
        .bind(actor.institution_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(sections)
    }

    /// The roster of one section with each enrollment's grade state.
    /// Instructors see only sections they are assigned to (anything else is
    /// 404, indistinguishable from nonexistent); a records officer sees any
    /// section in the institution.
    pub async fn roster(
        &self,
        actor: &Actor,
        section_id: Uuid,
    ) -> Result<Vec<RosterRow>, AppError> {
        let visible: bool = if actor.has_role(Role::RecordsOfficer) {
            sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM section WHERE id = $1 AND institution_id = $2)",
            )
            .bind(section_id)
            .bind(actor.institution_id)
            .fetch_one(&self.pool)
            .await?
        } else if actor.has_role(Role::Instructor) {
            sqlx::query_scalar(
                r#"
                SELECT EXISTS (
                    SELECT 1 FROM instructor_assignment ia
                    JOIN section s ON s.id = ia.section_id
                    WHERE ia.section_id = $1
                      AND ia.instructor_user_id = $2
                      AND s.institution_id = $3
                )
                "#,
            )
            .bind(section_id)
            .bind(actor.user_id)
            .bind(actor.institution_id)
            .fetch_one(&self.pool)
            .await?
        } else {
            return Err(AppError::Forbidden);
        };
        if !visible {
            return Err(AppError::NotFound);
        }

        let rows = sqlx::query_as::<_, RosterRow>(
            r#"
            SELECT
                e.id AS enrollment_id,
                sp.student_number,
                ua.username AS student_name,
                g.grade_code,
                g.state,
                g.version
            FROM enrollment e
            JOIN student_profile sp ON sp.id = e.student_id
            JOIN user_account ua ON ua.id = sp.user_id
            LEFT JOIN grade_record g ON g.enrollment_id = e.id
            WHERE e.section_id = $1
              AND e.status = 'enrolled'
            ORDER BY sp.student_number
            "#,
        )
        .bind(section_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn student_grades(
        &self,
        actor: &Actor,
        term_id: Uuid,
    ) -> Result<Vec<StudentGradeRow>, AppError> {
        let student_id = actor.require_student_self()?;

        let rows = sqlx::query_as::<_, StudentGradeRow>(
            r#"
            SELECT
                c.code AS course_code,
                c.title AS course_title,
                s.section_code,
                g.grade_code,
                g.published_at
            FROM grade_record g
            JOIN enrollment e ON e.id = g.enrollment_id
            JOIN section s ON s.id = e.section_id
            JOIN course c ON c.id = s.course_id
            WHERE e.student_id = $1
              AND s.term_id = $2
              AND g.institution_id = $3
              AND g.state IN ('published', 'amended')
            ORDER BY c.code, s.section_code
            "#,
        )
        .bind(student_id)
        .bind(term_id)
        .bind(actor.institution_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows)
    }

    /// The student's full academic history: every published or amended grade
    /// across all terms. Drafts are excluded by the query itself — there is
    /// no calling convention that returns them.
    pub async fn academic_history(&self, actor: &Actor) -> Result<Vec<HistoryRow>, AppError> {
        let student_id = actor.require_student_self()?;
        let rows = sqlx::query_as::<_, HistoryRow>(
            r#"
            SELECT
                t.code AS term_code,
                t.name AS term_name,
                c.code AS course_code,
                c.title AS course_title,
                c.credit_hours::float8 AS credit_hours,
                g.grade_code,
                g.grade_points,
                g.state
            FROM grade_record g
            JOIN enrollment e ON e.id = g.enrollment_id
            JOIN section s ON s.id = e.section_id
            JOIN academic_term t ON t.id = s.term_id
            JOIN course c ON c.id = s.course_id
            WHERE e.student_id = $1
              AND g.institution_id = $2
              AND g.state IN ('published', 'amended')
            ORDER BY t.starts_on, c.code
            "#,
        )
        .bind(student_id)
        .bind(actor.institution_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn save_draft(
        &self,
        actor: &Actor,
        command: SaveGradeCommand,
    ) -> Result<i64, AppError> {
        if !actor.has_role(Role::Instructor) && !actor.has_role(Role::RecordsOfficer) {
            return Err(AppError::Forbidden);
        }
        validate_grade_code(&command.grade_code)?;

        let mut tx = self.pool.begin().await?;

        // Resolve the enrollment inside the actor's institution first: a
        // cross-institution id answers 404 instead of silently writing
        // nothing, and the same query fetches the term's grade-entry window.
        let context = sqlx::query_as::<_, GradeEntryContext>(
            r#"
            SELECT e.section_id, t.grade_entry_closes_at
            FROM enrollment e
            JOIN section s ON s.id = e.section_id
            JOIN academic_term t ON t.id = s.term_id
            WHERE e.id = $1 AND e.institution_id = $2
            "#,
        )
        .bind(command.enrollment_id)
        .bind(actor.institution_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(AppError::NotFound)?;

        let is_officer = actor.has_role(Role::RecordsOfficer);

        // Instructors grade only sections they are assigned to — enforced
        // here in the service, not by which buttons a template shows.
        if !is_officer {
            let assigned: bool = sqlx::query_scalar(
                r#"
                SELECT EXISTS (
                    SELECT 1 FROM instructor_assignment
                    WHERE section_id = $1 AND instructor_user_id = $2
                )
                "#,
            )
            .bind(context.section_id)
            .bind(actor.user_id)
            .fetch_one(&mut *tx)
            .await?;
            if !assigned {
                return Err(AppError::Forbidden);
            }

            // The entry window binds instructors; the records officer is the
            // escape hatch for late entry (assumption A17). A term with no
            // deadline configured imposes none.
            if let Some(closes_at) = context.grade_entry_closes_at
                && Utc::now() >= closes_at
            {
                return Err(AppError::Conflict("grade entry window is closed".into()));
            }
        }

        let existing = sqlx::query_as::<_, ExistingGrade>(
            r#"
            SELECT id, grade_code, state, version
            FROM grade_record
            WHERE enrollment_id = $1
            FOR UPDATE
            "#,
        )
        .bind(command.enrollment_id)
        .fetch_optional(&mut *tx)
        .await?;

        let new_version = match existing {
            Some(old) => {
                // A published grade is a fact students have seen. Draft entry
                // cannot quietly rewrite or unpublish it — that is what the
                // correction workflow (state 'amended', reason, history) is
                // for.
                if old.state != "draft" {
                    return Err(AppError::Conflict(
                        "this grade is published; use a records-office correction".into(),
                    ));
                }
                if old.version != command.expected_version {
                    return Err(AppError::Conflict(
                        "grade was changed by another user".into(),
                    ));
                }

                let row: (i64,) = sqlx::query_as(
                    r#"
                    UPDATE grade_record
                       SET grade_code = $2,
                           grade_points = $3,
                           numeric_value = $4,
                           state = 'draft',
                           entered_by_user_id = $5,
                           version = version + 1,
                           updated_at = now()
                     WHERE id = $1
                    RETURNING version
                    "#,
                )
                .bind(old.id)
                .bind(&command.grade_code)
                .bind(command.grade_points)
                .bind(command.numeric_value)
                .bind(actor.user_id)
                .fetch_one(&mut *tx)
                .await?;

                self.audit
                    .write(
                        &mut tx,
                        actor.institution_id,
                        actor.user_id,
                        "grade.draft_changed",
                        "grade_record",
                        old.id,
                        &serde_json::json!({
                            "old_grade": old.grade_code,
                            "new_grade": command.grade_code,
                            "new_version": row.0
                        }),
                    )
                    .await?;

                row.0
            }
            None => {
                if command.expected_version != 0 {
                    return Err(AppError::Conflict(
                        "grade no longer has the expected state".into(),
                    ));
                }

                let id = Uuid::new_v4();
                sqlx::query(
                    r#"
                    INSERT INTO grade_record (
                        id, institution_id, enrollment_id, grade_code,
                        grade_points, numeric_value, state,
                        entered_by_user_id, version
                    )
                    SELECT $1, $2, e.id, $4, $5, $6, 'draft', $7, 1
                    FROM enrollment e
                    WHERE e.id = $3 AND e.institution_id = $2
                    "#,
                )
                .bind(id)
                .bind(actor.institution_id)
                .bind(command.enrollment_id)
                .bind(&command.grade_code)
                .bind(command.grade_points)
                .bind(command.numeric_value)
                .bind(actor.user_id)
                .execute(&mut *tx)
                .await?;

                self.audit
                    .write(
                        &mut tx,
                        actor.institution_id,
                        actor.user_id,
                        "grade.draft_created",
                        "grade_record",
                        id,
                        &serde_json::json!({
                            "enrollment_id": command.enrollment_id,
                            "grade": command.grade_code
                        }),
                    )
                    .await?;

                1
            }
        };

        tx.commit().await?;
        Ok(new_version)
    }

    /// Records-office correction of a published grade. The database trigger
    /// preserves the prior value and its author in `grade_revision`; the
    /// state becomes `amended` and the reason is audited in the same
    /// transaction.
    pub async fn correct_grade(
        &self,
        actor: &Actor,
        command: CorrectGradeCommand,
    ) -> Result<i64, AppError> {
        if !actor.has_role(Role::RecordsOfficer) {
            return Err(AppError::Forbidden);
        }
        validate_grade_code(&command.grade_code)?;
        let reason = command.reason.trim();
        if reason.is_empty() {
            return Err(AppError::Validation(
                "a grade correction requires a reason".into(),
            ));
        }

        let mut tx = self.pool.begin().await?;

        let existing = sqlx::query_as::<_, ExistingGrade>(
            r#"
            SELECT g.id, g.grade_code, g.state, g.version
            FROM grade_record g
            WHERE g.enrollment_id = $1 AND g.institution_id = $2
            FOR UPDATE
            "#,
        )
        .bind(command.enrollment_id)
        .bind(actor.institution_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(AppError::NotFound)?;

        if existing.state == "draft" {
            return Err(AppError::Conflict(
                "draft grades are edited by entry, not corrected".into(),
            ));
        }
        if existing.version != command.expected_version {
            return Err(AppError::Conflict(
                "grade was changed by another user".into(),
            ));
        }

        let new_version: i64 = sqlx::query_scalar(
            r#"
            UPDATE grade_record
               SET grade_code = $2,
                   grade_points = $3,
                   numeric_value = $4,
                   state = 'amended',
                   entered_by_user_id = $5,
                   version = version + 1,
                   updated_at = now()
             WHERE id = $1
            RETURNING version
            "#,
        )
        .bind(existing.id)
        .bind(&command.grade_code)
        .bind(command.grade_points)
        .bind(command.numeric_value)
        .bind(actor.user_id)
        .fetch_one(&mut *tx)
        .await?;

        self.audit
            .write(
                &mut tx,
                actor.institution_id,
                actor.user_id,
                "grade.corrected",
                "grade_record",
                existing.id,
                &serde_json::json!({
                    "old_grade": existing.grade_code,
                    "new_grade": command.grade_code,
                    "reason": reason,
                    "new_version": new_version,
                }),
            )
            .await?;

        tx.commit().await?;
        Ok(new_version)
    }

    pub async fn publish_section(&self, actor: &Actor, section_id: Uuid) -> Result<u64, AppError> {
        if !actor.has_role(Role::RecordsOfficer) {
            return Err(AppError::Forbidden);
        }

        let mut tx = self.pool.begin().await?;

        let changed = sqlx::query(
            r#"
            UPDATE grade_record g
               SET state = 'published',
                   published_at = now(),
                   version = version + 1,
                   updated_at = now()
              FROM enrollment e
             WHERE g.enrollment_id = e.id
               AND e.section_id = $1
               AND g.institution_id = $2
               AND g.state = 'draft'
            "#,
        )
        .bind(section_id)
        .bind(actor.institution_id)
        .execute(&mut *tx)
        .await?;

        self.audit
            .write(
                &mut tx,
                actor.institution_id,
                actor.user_id,
                "grade.section_published",
                "section",
                section_id,
                &serde_json::json!({ "count": changed.rows_affected() }),
            )
            .await?;

        tx.commit().await?;
        Ok(changed.rows_affected())
    }
}

#[derive(sqlx::FromRow)]
struct ExistingGrade {
    id: Uuid,
    grade_code: String,
    state: String,
    version: i64,
}

#[derive(sqlx::FromRow)]
struct GradeEntryContext {
    section_id: Uuid,
    grade_entry_closes_at: Option<DateTime<Utc>>,
}

/// Grade codes are short registrar-defined tokens ("A", "B+", "INC").
fn validate_grade_code(code: &str) -> Result<(), AppError> {
    if code.trim().is_empty() || code.len() > 8 {
        return Err(AppError::Validation(
            "grade code must be 1–8 characters".into(),
        ));
    }
    Ok(())
}
