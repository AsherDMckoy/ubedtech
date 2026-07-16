use crate::audit::AuditWriter;
use crate::shared::{actor::Actor, error::AppError};
use chrono::Utc;
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::enrollment::{
    policy::{require_can_grant_override, require_can_register_for},
    types::{
        Denial, EnrollError, EnrollmentReceipt, GrantOverrideCommand, RegisterCommand,
        RegistrationContext,
    },
};

/// The rules a registrar may lift with `registration_override` rows.
pub const OVERRIDE_TYPES: [&str; 5] = [
    "hold",
    "prerequisite",
    "schedule_conflict",
    "capacity",
    "deadline",
];

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
    ) -> Result<EnrollmentReceipt, EnrollError> {
        let student_id = actor.require_student_self()?;
        self.register_for(actor, student_id, command).await
    }

    pub async fn register_for(
        &self,
        actor: &Actor,
        student_id: Uuid,
        command: RegisterCommand,
    ) -> Result<EnrollmentReceipt, EnrollError> {
        require_can_register_for(actor, student_id)?;

        // Overrides claimed (row-locked) while checks pass; stamped as
        // consumed by the new enrollment right after it exists. A rollback
        // releases the locks, so a failed registration never burns one.
        let mut consumed_override_ids: Vec<Uuid> = Vec::new();

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
                t.add_drop_closes_at
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
            return Err(EnrollError::Denied(Denial::SectionNotOpen));
        }

        // One shared deadline governs adds and drops (ADR-8): adds are allowed
        // from registration_opens_at until add_drop_closes_at. A registrar
        // `deadline` override lifts the closing deadline for a late add; it
        // never opens registration early.
        let now = Utc::now();
        if now < context.registration_opens_at {
            return Err(EnrollError::Denied(Denial::WindowClosed));
        }
        if now >= context.add_drop_closes_at {
            match claim_override(
                &mut tx,
                actor.institution_id,
                student_id,
                context.term_id,
                Some(command.section_id),
                "deadline",
            )
            .await?
            {
                Some(override_id) => consumed_override_ids.push(override_id),
                None => return Err(EnrollError::Denied(Denial::WindowClosed)),
            }
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
            match claim_override(
                &mut tx,
                actor.institution_id,
                student_id,
                context.term_id,
                Some(command.section_id),
                "hold",
            )
            .await?
            {
                Some(override_id) => consumed_override_ids.push(override_id),
                None => return Err(EnrollError::Denied(Denial::Hold)),
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
            return Err(EnrollError::Denied(Denial::AlreadyEnrolled));
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

        if !prerequisites_met {
            match claim_override(
                &mut tx,
                actor.institution_id,
                student_id,
                context.term_id,
                Some(command.section_id),
                "prerequisite",
            )
            .await?
            {
                Some(override_id) => consumed_override_ids.push(override_id),
                None => return Err(EnrollError::Denied(Denial::PrerequisiteNotMet)),
            }
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

        if has_time_conflict {
            match claim_override(
                &mut tx,
                actor.institution_id,
                student_id,
                context.term_id,
                Some(command.section_id),
                "schedule_conflict",
            )
            .await?
            {
                Some(override_id) => consumed_override_ids.push(override_id),
                None => return Err(EnrollError::Denied(Denial::ScheduleConflict)),
            }
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

        if reserved.is_none() {
            // Migration 0010's trigger guarantees a capacity row for every
            // section. If it is gone anyway (manual deletion, corruption),
            // that is a broken invariant, not a full section — fail loudly
            // and distinctly rather than telling the student "section is
            // full" about a section that has no capacity record at all.
            let capacity_row_exists: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM section_capacity WHERE section_id = $1)",
            )
            .bind(command.section_id)
            .fetch_one(&mut *tx)
            .await?;
            if !capacity_row_exists {
                return Err(EnrollError::App(AppError::Integrity(
                    "section has no section_capacity row; the migration-0010 guarantee was bypassed",
                )));
            }

            // A registrar-granted capacity override admits one student past a
            // full section by raising capacity and enrolled_count together, so
            // the enrolled_count <= capacity constraint keeps holding and no
            // unauthorized seat opens up for anyone else.
            match claim_override(
                &mut tx,
                actor.institution_id,
                student_id,
                context.term_id,
                Some(command.section_id),
                "capacity",
            )
            .await?
            {
                Some(override_id) => {
                    consumed_override_ids.push(override_id);
                    sqlx::query(
                        r#"
                        UPDATE section_capacity
                           SET capacity = capacity + 1,
                               enrolled_count = enrolled_count + 1,
                               version = version + 1
                         WHERE section_id = $1
                        "#,
                    )
                    .bind(command.section_id)
                    .execute(&mut *tx)
                    .await?;
                }
                None => return Err(EnrollError::Denied(Denial::SectionFull)),
            }
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

        stamp_overrides_consumed(&mut tx, &consumed_override_ids, enrollment_id).await?;

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
                    overrides_consumed: &consumed_override_ids,
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

    pub async fn drop_self(&self, actor: &Actor, enrollment_id: Uuid) -> Result<(), EnrollError> {
        let student_id = actor.require_student_self()?;
        self.drop_for(actor, student_id, enrollment_id).await
    }

    pub async fn drop_for(
        &self,
        actor: &Actor,
        student_id: Uuid,
        enrollment_id: Uuid,
    ) -> Result<(), EnrollError> {
        require_can_register_for(actor, student_id)?;

        let mut consumed_override_ids: Vec<Uuid> = Vec::new();

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
                t.add_drop_closes_at
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

        if Utc::now() >= row.add_drop_closes_at {
            match claim_override(
                &mut tx,
                actor.institution_id,
                student_id,
                row.term_id,
                Some(row.section_id),
                "deadline",
            )
            .await?
            {
                Some(override_id) => consumed_override_ids.push(override_id),
                None => return Err(EnrollError::Denied(Denial::WindowClosed)),
            }
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

        // If this enrollment got in on a capacity override, the extra seat
        // was that student's, not the section's: dropping reverts the
        // capacity bump too, so no unauthorized seat leaks into the pool.
        let consumed_capacity_override: bool = sqlx::query_scalar(
            r#"
            SELECT EXISTS (
                SELECT 1 FROM registration_override
                WHERE consumed_by_enrollment_id = $1 AND override_type = 'capacity'
            )
            "#,
        )
        .bind(enrollment_id)
        .fetch_one(&mut *tx)
        .await?;

        let changed = sqlx::query(
            r#"
            UPDATE section_capacity
               SET enrolled_count = enrolled_count - 1,
                   capacity = capacity - $2,
                   version = version + 1
             WHERE section_id = $1
               AND enrolled_count > 0
               AND capacity >= $2
            "#,
        )
        .bind(row.section_id)
        .bind(i32::from(consumed_capacity_override))
        .execute(&mut *tx)
        .await?;

        if changed.rows_affected() != 1 {
            return Err(EnrollError::App(AppError::Integrity(
                "drop could not release a seat: capacity row missing or enrolled_count drifted",
            )));
        }

        stamp_overrides_consumed(&mut tx, &consumed_override_ids, enrollment_id).await?;

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
                    "section_id": row.section_id,
                    "overrides_consumed": consumed_override_ids,
                }),
            )
            .await?;

        tx.commit().await?;
        Ok(())
    }

    /// Registrar-granted override: the full record CLAUDE.md §1 item 6
    /// demands — who grants it (the actor, audited), which rule it lifts,
    /// why (required), when it expires, and — once used — which enrollment
    /// consumed it (`stamp_overrides_consumed`).
    pub async fn grant_override(
        &self,
        actor: &Actor,
        student_id: Uuid,
        command: GrantOverrideCommand,
    ) -> Result<Uuid, AppError> {
        require_can_grant_override(actor)?;

        if !OVERRIDE_TYPES.contains(&command.override_type.as_str()) {
            return Err(AppError::Validation(format!(
                "unknown override type: {}",
                command.override_type
            )));
        }
        let reason = command.reason.trim();
        if reason.is_empty() {
            return Err(AppError::Validation("an override requires a reason".into()));
        }
        if let Some(expires_at) = command.expires_at
            && expires_at <= Utc::now()
        {
            return Err(AppError::Validation(
                "override expiry must be in the future".into(),
            ));
        }

        let mut tx = self.pool.begin().await?;

        // Targets resolve inside the actor's institution only; anything
        // outside it answers 404, exactly like the identity admin routes.
        let student_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM student_profile WHERE id = $1 AND institution_id = $2)",
        )
        .bind(student_id)
        .bind(actor.institution_id)
        .fetch_one(&mut *tx)
        .await?;
        if !student_exists {
            return Err(AppError::NotFound);
        }

        let term_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM academic_term WHERE id = $1 AND institution_id = $2)",
        )
        .bind(command.term_id)
        .bind(actor.institution_id)
        .fetch_one(&mut *tx)
        .await?;
        if !term_exists {
            return Err(AppError::NotFound);
        }

        if let Some(section_id) = command.section_id {
            let section_in_term: Option<bool> = sqlx::query_scalar(
                "SELECT term_id = $2 FROM section WHERE id = $1 AND institution_id = $3",
            )
            .bind(section_id)
            .bind(command.term_id)
            .bind(actor.institution_id)
            .fetch_optional(&mut *tx)
            .await?;
            match section_in_term {
                None => return Err(AppError::NotFound),
                Some(false) => {
                    return Err(AppError::Validation(
                        "section does not belong to the term".into(),
                    ));
                }
                Some(true) => {}
            }
        }

        let override_id = Uuid::new_v4();
        sqlx::query(
            r#"
            INSERT INTO registration_override (
                id, institution_id, student_id, term_id, section_id,
                override_type, granted_by_user_id, expires_at, note
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            "#,
        )
        .bind(override_id)
        .bind(actor.institution_id)
        .bind(student_id)
        .bind(command.term_id)
        .bind(command.section_id)
        .bind(&command.override_type)
        .bind(actor.user_id)
        .bind(command.expires_at)
        .bind(reason)
        .execute(&mut *tx)
        .await?;

        self.audit
            .write(
                &mut tx,
                actor.institution_id,
                actor.user_id,
                "enrollment.override_granted",
                "registration_override",
                override_id,
                &serde_json::json!({
                    "student_id": student_id,
                    "term_id": command.term_id,
                    "section_id": command.section_id,
                    "override_type": command.override_type,
                    "expires_at": command.expires_at,
                }),
            )
            .await?;

        tx.commit().await?;
        Ok(override_id)
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
    add_drop_closes_at: chrono::DateTime<Utc>,
}

#[derive(Serialize)]
struct RegistrationAudit<'a> {
    student_id: Uuid,
    section_id: Uuid,
    source: &'a str,
    overrides_consumed: &'a [Uuid],
}

/// Lock one usable override (right rule, right student/term, unconsumed,
/// unexpired, institution-scoped) and return its id. Section-specific
/// overrides are preferred over term-wide ones. `FOR UPDATE SKIP LOCKED`
/// means concurrent transactions never block here and can never claim the
/// same row — an override is consumed by at most one enrollment transaction.
/// The row lock (not a write) does the claiming, so a transaction that later
/// fails releases the override untouched.
async fn claim_override(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    institution_id: Uuid,
    student_id: Uuid,
    term_id: Uuid,
    section_id: Option<Uuid>,
    override_type: &str,
) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar(
        r#"
        SELECT id
        FROM registration_override
        WHERE institution_id = $1
          AND student_id = $2
          AND term_id = $3
          AND (section_id IS NULL OR section_id = $4)
          AND override_type = $5
          AND consumed_at IS NULL
          AND (expires_at IS NULL OR expires_at > now())
        ORDER BY section_id NULLS LAST, created_at
        LIMIT 1
        FOR UPDATE SKIP LOCKED
        "#,
    )
    .bind(institution_id)
    .bind(student_id)
    .bind(term_id)
    .bind(section_id)
    .bind(override_type)
    .fetch_optional(&mut **tx)
    .await
}

/// Record which enrollment transaction consumed each claimed override — in
/// the same transaction, so the record and the enrollment commit or roll
/// back together.
async fn stamp_overrides_consumed(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    override_ids: &[Uuid],
    enrollment_id: Uuid,
) -> Result<(), sqlx::Error> {
    if override_ids.is_empty() {
        return Ok(());
    }
    sqlx::query(
        r#"
        UPDATE registration_override
           SET consumed_at = now(),
               consumed_by_enrollment_id = $2
         WHERE id = ANY($1)
        "#,
    )
    .bind(override_ids)
    .bind(enrollment_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
