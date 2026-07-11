use crate::audit::AuditWriter;
use chrono::{DateTime, Utc};
use crate::shared::{actor::{Actor, Role}, error::AppError};
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

impl GradeService {
    pub fn new(pool: PgPool, audit: AuditWriter) -> Self {
        Self { pool, audit }
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

    pub async fn save_draft(
        &self,
        actor: &Actor,
        command: SaveGradeCommand,
    ) -> Result<i64, AppError> {
        if !actor.has_role(Role::Instructor) && !actor.has_role(Role::RecordsOfficer) {
            return Err(AppError::Forbidden);
        }

        let mut tx = self.pool.begin().await?;

        let assignment_allowed: bool = if actor.has_role(Role::RecordsOfficer) {
            true
        } else {
            sqlx::query_scalar(
                r#"
                SELECT EXISTS (
                    SELECT 1
                    FROM enrollment e
                    JOIN instructor_assignment ia ON ia.section_id = e.section_id
                    WHERE e.id = $1
                      AND ia.instructor_user_id = $2
                )
                "#,
            )
            .bind(command.enrollment_id)
            .bind(actor.user_id)
            .fetch_one(&mut *tx)
            .await?
        };

        if !assignment_allowed {
            return Err(AppError::Forbidden);
        }

        let existing = sqlx::query_as::<_, ExistingGrade>(
            r#"
            SELECT id, grade_code, version
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

    pub async fn publish_section(
        &self,
        actor: &Actor,
        section_id: Uuid,
    ) -> Result<u64, AppError> {
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
    version: i64,
}
