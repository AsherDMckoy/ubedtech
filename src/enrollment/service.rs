use crate::audit::AuditWriter;
use crate::shared::{actor::Actor, error::AppError};
use chrono::Utc;
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::enrollment::{
    policy::require_can_register_for,
    types::{EnrollmentReceipt, RegisterCommand, RegistrationContext},
};

#[derive(Clone)]
pub struct EnrollmentService {
    pool: PgPool,
    audit: AuditWriter,
}

impl EnrollmentService {
    pub fn new(pool: PgPool, audit: AuditWriter) -> Self {
        Self { pool, audit }
    }

    pub async fn register_self(
        &self,
        actor: &Actor,
        command: RegisterCommand,
    ) -> Result<EnrollmentReceipt, AppError> {
        let student_id = actor.require_student_self()?;
        self.register_for(actor, student_id, command).await
    }

    pub async fn register_for(
        &self,
        actor: &Actor,
        student_id: Uuid,
        command: RegisterCommand,
    ) -> Result<EnrollmentReceipt, AppError> {
        require_can_register_for(actor, student_id)?;

        let mut tx = self.pool.begin().await?;

        // Idempotency is checked before doing expensive work. A repeated browser
        // submission returns the original successful result.
        if let Some(existing) = sqlx::query_as::<_, EnrollmentReceipt>(
            r#"
            SELECT
                id AS enrollment_id,
                section_id,
                status::text AS status,
                registered_at
            FROM enrollment
            WHERE institution_id = $1
              AND student_id = $2
              AND idempotency_key = $3
            "#,
        )
        .bind(actor.institution_id)
        .bind(student_id)
        .bind(command.idempotency_key)
        .fetch_optional(&mut *tx)
        .await?
        {
            tx.commit().await?;
            return Ok(existing);
        }

        let context = sqlx::query_as::<_, RegistrationContext>(
            r#"
            SELECT
                s.term_id,
                s.course_id,
                s.status AS section_status,
                t.registration_opens_at,
                t.registration_closes_at,
                t.drop_add_closes_at
            FROM section s
            JOIN academic_term t ON t.id = s.term_id
            WHERE s.id = $1
              AND s.institution_id = $2
            "#,
        )
        .bind(command.section_id)
        .bind(actor.institution_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(AppError::NotFound)?;

        if context.section_status != "open" {
            return Err(AppError::Conflict("section is not open".into()));
        }

        let now = Utc::now();
        if now < context.registration_opens_at || now >= context.registration_closes_at {
            return Err(AppError::Conflict("registration window is closed".into()));
        }

        // Ensure the lock row exists, then lock it. All registration changes for
        // one student and term now execute in a clear order.
        sqlx::query(
            r#"
            INSERT INTO student_term_registration (student_id, term_id)
            VALUES ($1, $2)
            ON CONFLICT (student_id, term_id) DO NOTHING
            "#,
        )
        .bind(student_id)
        .bind(context.term_id)
        .execute(&mut *tx)
        .await?;

        let term_state = sqlx::query_as::<_, StudentTermState>(
            r#"
            SELECT status, hold_flags
            FROM student_term_registration
            WHERE student_id = $1 AND term_id = $2
            FOR UPDATE
            "#,
        )
        .bind(student_id)
        .bind(context.term_id)
        .fetch_one(&mut *tx)
        .await?;

        if term_state.status != "eligible" || !term_state.hold_flags.is_empty() {
            let has_override: bool = sqlx::query_scalar(
                r#"
                SELECT EXISTS (
                    SELECT 1
                    FROM registration_override
                    WHERE student_id = $1
                      AND term_id = $2
                      AND override_type = 'hold'
                      AND (expires_at IS NULL OR expires_at > now())
                )
                "#,
            )
            .bind(student_id)
            .bind(context.term_id)
            .fetch_one(&mut *tx)
            .await?;

            if !has_override {
                return Err(AppError::Conflict("student has a registration hold".into()));
            }
        }

        // Repeat the idempotency lookup after acquiring the per-student/term lock.
        // Two identical submissions can pass the fast pre-lock lookup together;
        // this second lookup makes the later request return the first result.
        if let Some(existing) = sqlx::query_as::<_, EnrollmentReceipt>(
            r#"
            SELECT
                id AS enrollment_id,
                section_id,
                status::text AS status,
                registered_at
            FROM enrollment
            WHERE institution_id = $1
              AND student_id = $2
              AND idempotency_key = $3
            "#,
        )
        .bind(actor.institution_id)
        .bind(student_id)
        .bind(command.idempotency_key)
        .fetch_optional(&mut *tx)
        .await?
        {
            tx.commit().await?;
            return Ok(existing);
        }

        let duplicate: bool = sqlx::query_scalar(
            r#"
            SELECT EXISTS (
                SELECT 1
                FROM enrollment
                WHERE student_id = $1
                  AND section_id = $2
                  AND status = 'enrolled'
            )
            "#,
        )
        .bind(student_id)
        .bind(command.section_id)
        .fetch_one(&mut *tx)
        .await?;

        if duplicate {
            return Err(AppError::Conflict(
                "student is already enrolled in this section".into(),
            ));
        }

        let prerequisites_met: bool = sqlx::query_scalar(
            r#"
            SELECT NOT EXISTS (
                SELECT 1
                FROM course_prerequisite p
                WHERE p.course_id = $2
                  AND NOT EXISTS (
                      SELECT 1
                      FROM records_student_course_completion c
                      WHERE c.student_id = $1
                        AND c.course_id = p.prerequisite_course_id
                        AND c.best_grade_points >= p.minimum_grade_points
                  )
            )
            "#,
        )
        .bind(student_id)
        .bind(context.course_id)
        .fetch_one(&mut *tx)
        .await?;

        if !prerequisites_met
            && !has_override(
                &mut tx,
                student_id,
                context.term_id,
                Some(command.section_id),
                "prerequisite",
            )
            .await?
        {
            return Err(AppError::Conflict(
                "prerequisite requirements are not satisfied".into(),
            ));
        }

        let has_time_conflict: bool = sqlx::query_scalar(
            r#"
            SELECT EXISTS (
                SELECT 1
                FROM enrollment e
                JOIN section existing_section ON existing_section.id = e.section_id
                JOIN section_meeting existing_meeting
                  ON existing_meeting.section_id = existing_section.id
                JOIN section_meeting target_meeting
                  ON target_meeting.section_id = $2
                 AND target_meeting.day_of_week = existing_meeting.day_of_week
                 AND target_meeting.starts_at < existing_meeting.ends_at
                 AND existing_meeting.starts_at < target_meeting.ends_at
                WHERE e.student_id = $1
                  AND e.status = 'enrolled'
                  AND existing_section.term_id = $3
            )
            "#,
        )
        .bind(student_id)
        .bind(command.section_id)
        .bind(context.term_id)
        .fetch_one(&mut *tx)
        .await?;

        if has_time_conflict
            && !has_override(
                &mut tx,
                student_id,
                context.term_id,
                Some(command.section_id),
                "schedule_conflict",
            )
            .await?
        {
            return Err(AppError::Conflict("schedule conflict detected".into()));
        }

        // The conditional update is the seat reservation algorithm. It does not
        // read a count and later write it; that would race under concurrency.
        let reserved = sqlx::query_scalar::<_, i32>(
            r#"
            UPDATE section_capacity
               SET enrolled_count = enrolled_count + 1,
                   version = version + 1
             WHERE section_id = $1
               AND enrolled_count < capacity
            RETURNING enrolled_count
            "#,
        )
        .bind(command.section_id)
        .fetch_optional(&mut *tx)
        .await?;

        if reserved.is_none()
            && !has_override(
                &mut tx,
                student_id,
                context.term_id,
                Some(command.section_id),
                "capacity",
            )
            .await?
        {
            return Err(AppError::Conflict("section is full".into()));
        }

        // A capacity override needs an explicit capacity policy. For the demo,
        // it does not silently increase the counter beyond the database check.
        if reserved.is_none() {
            return Err(AppError::Conflict(
                "capacity override requires registrar seat adjustment".into(),
            ));
        }

        let enrollment_id = Uuid::new_v4();
        let registered_at = Utc::now();
        let source = if actor.student_id == Some(student_id) {
            "student"
        } else {
            "registrar"
        };

        sqlx::query(
            r#"
            INSERT INTO enrollment (
                id, institution_id, student_id, section_id, status,
                registered_at, source, idempotency_key, created_by_user_id
            )
            VALUES ($1, $2, $3, $4, 'enrolled', $5, $6, $7, $8)
            "#,
        )
        .bind(enrollment_id)
        .bind(actor.institution_id)
        .bind(student_id)
        .bind(command.section_id)
        .bind(registered_at)
        .bind(source)
        .bind(command.idempotency_key)
        .bind(actor.user_id)
        .execute(&mut *tx)
        .await?;

        self.audit
            .write(
                &mut tx,
                actor.institution_id,
                actor.user_id,
                "enrollment.registered",
                "enrollment",
                enrollment_id,
                &RegistrationAudit {
                    student_id,
                    section_id: command.section_id,
                    source,
                },
            )
            .await?;

        tx.commit().await?;

        Ok(EnrollmentReceipt {
            enrollment_id,
            section_id: command.section_id,
            status: "enrolled".into(),
            registered_at,
        })
    }

    pub async fn drop_self(&self, actor: &Actor, enrollment_id: Uuid) -> Result<(), AppError> {
        let student_id = actor.require_student_self()?;
        self.drop_for(actor, student_id, enrollment_id).await
    }

    pub async fn drop_for(
        &self,
        actor: &Actor,
        student_id: Uuid,
        enrollment_id: Uuid,
    ) -> Result<(), AppError> {
        require_can_register_for(actor, student_id)?;

        let mut tx = self.pool.begin().await?;

        // Read enough context to identify the term, but do not lock the
        // enrollment first. Registration and drop must share one lock order:
        // student-term -> enrollment/section state. A fixed order removes an
        // avoidable source of deadlocks under registration-period contention.
        let row = sqlx::query_as::<_, DropContext>(
            r#"
            SELECT
                e.section_id,
                s.term_id,
                t.drop_add_closes_at
            FROM enrollment e
            JOIN section s ON s.id = e.section_id
            JOIN academic_term t ON t.id = s.term_id
            WHERE e.id = $1
              AND e.student_id = $2
              AND e.institution_id = $3
              AND e.status = 'enrolled'
            "#,
        )
        .bind(enrollment_id)
        .bind(student_id)
        .bind(actor.institution_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(AppError::NotFound)?;

        sqlx::query(
            r#"
            INSERT INTO student_term_registration (student_id, term_id)
            VALUES ($1, $2)
            ON CONFLICT (student_id, term_id) DO NOTHING
            "#,
        )
        .bind(student_id)
        .bind(row.term_id)
        .execute(&mut *tx)
        .await?;

        sqlx::query_scalar::<_, i32>(
            r#"
            SELECT 1
            FROM student_term_registration
            WHERE student_id = $1 AND term_id = $2
            FOR UPDATE
            "#,
        )
        .bind(student_id)
        .bind(row.term_id)
        .fetch_one(&mut *tx)
        .await?;

        // Re-check the mutable state after obtaining the serialization lock.
        // A concurrent duplicate drop will now observe the committed state and
        // fail rather than decrementing capacity twice.
        sqlx::query_scalar::<_, i32>(
            r#"
            SELECT 1
            FROM enrollment
            WHERE id = $1
              AND student_id = $2
              AND institution_id = $3
              AND status = 'enrolled'
            FOR UPDATE
            "#,
        )
        .bind(enrollment_id)
        .bind(student_id)
        .bind(actor.institution_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(AppError::NotFound)?;

        if Utc::now() >= row.drop_add_closes_at
            && !has_override(
                &mut tx,
                student_id,
                row.term_id,
                Some(row.section_id),
                "deadline",
            )
            .await?
        {
            return Err(AppError::Conflict("drop/add period is closed".into()));
        }

        sqlx::query(
            r#"
            UPDATE enrollment
               SET status = 'dropped',
                   dropped_at = now(),
                   updated_at = now()
             WHERE id = $1
            "#,
        )
        .bind(enrollment_id)
        .execute(&mut *tx)
        .await?;

        let changed = sqlx::query(
            r#"
            UPDATE section_capacity
               SET enrolled_count = enrolled_count - 1,
                   version = version + 1
             WHERE section_id = $1
               AND enrolled_count > 0
            "#,
        )
        .bind(row.section_id)
        .execute(&mut *tx)
        .await?;

        if changed.rows_affected() != 1 {
            return Err(AppError::Internal);
        }

        self.audit
            .write(
                &mut tx,
                actor.institution_id,
                actor.user_id,
                "enrollment.dropped",
                "enrollment",
                enrollment_id,
                &serde_json::json!({
                    "student_id": student_id,
                    "section_id": row.section_id
                }),
            )
            .await?;

        tx.commit().await?;
        Ok(())
    }
}

#[derive(sqlx::FromRow)]
struct StudentTermState {
    status: String,
    hold_flags: Vec<String>,
}

#[derive(sqlx::FromRow)]
struct DropContext {
    section_id: Uuid,
    term_id: Uuid,
    drop_add_closes_at: chrono::DateTime<Utc>,
}

#[derive(Serialize)]
struct RegistrationAudit<'a> {
    student_id: Uuid,
    section_id: Uuid,
    source: &'a str,
}

async fn has_override(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    student_id: Uuid,
    term_id: Uuid,
    section_id: Option<Uuid>,
    override_type: &str,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM registration_override
            WHERE student_id = $1
              AND term_id = $2
              AND (section_id IS NULL OR section_id = $3)
              AND override_type = $4
              AND (expires_at IS NULL OR expires_at > now())
        )
        "#,
    )
    .bind(student_id)
    .bind(term_id)
    .bind(section_id)
    .bind(override_type)
    .fetch_one(&mut **tx)
    .await
}
