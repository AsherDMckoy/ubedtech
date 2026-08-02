use crate::audit::AuditWriter;
use crate::shared::{actor::Actor, error::AppError};
use chrono::Utc;
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::enrollment::{
    policy::{require_can_grant_override, require_can_manage_holds, require_can_register_for},
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

        // A course already completed with a passing grade cannot be taken
        // again. Passing = best published grade above 0.0 points, so a
        // failed course (F, 0.0) stays retakeable. Conservative default,
        // no override path — recorded as assumption A37.
        let already_completed: bool = sqlx::query_scalar(
            r#"
            SELECT EXISTS (
                SELECT 1
                FROM records_student_course_completion c
                WHERE c.student_id = $1
                  AND c.course_id = $2
                  AND c.best_grade_points > 0
            )
            "#,
        )
        .bind(student_id)
        .bind(context.course_id)
        .fetch_one(&mut *tx)
        .await?;

        if already_completed {
            return Err(EnrollError::Denied(Denial::CourseAlreadyCompleted));
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
        let source = if actor.student_id == Some(student_id) {
            "student"
        } else {
            "registrar"
        };

        // RETURNING gives back the timestamp as PostgreSQL stored it
        // (microseconds). Building the receipt from a Rust-side Utc::now()
        // would carry nanoseconds the database truncates, so an idempotent
        // retry — which reads the row back — would not equal the original.
        let registered_at: chrono::DateTime<Utc> = sqlx::query_scalar(
            r#"
            INSERT INTO enrollment (
                id, institution_id, student_id, section_id, status,
                registered_at, source, idempotency_key, created_by_user_id
            )
            VALUES ($1, $2, $3, $4, 'enrolled', now(), $5, $6, $7)
            RETURNING registered_at
            "#,
        )
        .bind(enrollment_id)
        .bind(actor.institution_id)
        .bind(student_id)
        .bind(command.section_id)
        .bind(source)
        .bind(command.idempotency_key)
        .bind(actor.user_id)
        .fetch_one(&mut *tx)
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

    /// Place a registration hold flag on a student's term. Idempotent: an
    /// already-present flag is a no-op with no audit row, so retries are
    /// side-effect-free.
    pub async fn place_hold(
        &self,
        actor: &Actor,
        student_id: Uuid,
        term_id: Uuid,
        flag: &str,
        reason: &str,
    ) -> Result<(), AppError> {
        require_can_manage_holds(actor)?;
        let flag = valid_hold_flag(flag)?;
        if reason.trim().is_empty() {
            return Err(AppError::Validation("a hold requires a reason".into()));
        }

        let mut tx = self.pool.begin().await?;
        self.check_hold_target(&mut tx, actor, student_id, term_id)
            .await?;

        sqlx::query(
            "INSERT INTO student_term_registration (student_id, term_id) VALUES ($1, $2) \
             ON CONFLICT (student_id, term_id) DO NOTHING",
        )
        .bind(student_id)
        .bind(term_id)
        .execute(&mut *tx)
        .await?;

        let added = sqlx::query(
            r#"
            UPDATE student_term_registration
               SET hold_flags = hold_flags || $3, updated_at = now()
             WHERE student_id = $1 AND term_id = $2 AND NOT ($3 = ANY(hold_flags))
            "#,
        )
        .bind(student_id)
        .bind(term_id)
        .bind(flag)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if added == 0 {
            return Ok(()); // flag already present
        }

        self.audit
            .write(
                &mut tx,
                actor.institution_id,
                actor.user_id,
                "enrollment.hold_placed",
                "student_profile",
                student_id,
                &serde_json::json!({ "term_id": term_id, "flag": flag, "reason": reason.trim() }),
            )
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Remove a hold flag. Idempotent like `place_hold`.
    pub async fn release_hold(
        &self,
        actor: &Actor,
        student_id: Uuid,
        term_id: Uuid,
        flag: &str,
    ) -> Result<(), AppError> {
        require_can_manage_holds(actor)?;
        let flag = valid_hold_flag(flag)?;

        let mut tx = self.pool.begin().await?;
        self.check_hold_target(&mut tx, actor, student_id, term_id)
            .await?;

        let removed = sqlx::query(
            r#"
            UPDATE student_term_registration
               SET hold_flags = array_remove(hold_flags, $3), updated_at = now()
             WHERE student_id = $1 AND term_id = $2 AND $3 = ANY(hold_flags)
            "#,
        )
        .bind(student_id)
        .bind(term_id)
        .bind(flag)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if removed == 0 {
            return Ok(());
        }

        self.audit
            .write(
                &mut tx,
                actor.institution_id,
                actor.user_id,
                "enrollment.hold_released",
                "student_profile",
                student_id,
                &serde_json::json!({ "term_id": term_id, "flag": flag }),
            )
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// The student's own active enrollments for one term, for the
    /// registration panel.
    pub async fn list_own_active(
        &self,
        actor: &Actor,
        term_id: Uuid,
    ) -> Result<Vec<crate::enrollment::types::EnrolledSection>, AppError> {
        let student_id = actor.require_student_self()?;
        let rows = sqlx::query_as::<_, crate::enrollment::types::EnrolledSection>(
            r#"
            SELECT
                e.id AS enrollment_id,
                c.code AS course_code,
                c.title AS course_title,
                s.section_code,
                c.credit_hours::float8 AS credit_hours,
                COALESCE(m.summary, '') AS meetings,
                COALESCE(i.names, '') AS instructors,
                COALESCE(p.codes, '') AS prerequisites,
                c.description,
                c.faculty
            FROM enrollment e
            JOIN section s ON s.id = e.section_id
            JOIN course c ON c.id = s.course_id
            LEFT JOIN LATERAL (
                SELECT string_agg(
                           (ARRAY['Mon','Tue','Wed','Thu','Fri','Sat','Sun'])[sm.day_of_week]
                           || ' ' || to_char(sm.starts_at, 'HH24:MI')
                           || '-' || to_char(sm.ends_at, 'HH24:MI')
                           || COALESCE(' · ' || r.room_code, ''),
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
            WHERE e.student_id = $1
              AND e.institution_id = $2
              AND e.status = 'enrolled'
              AND s.term_id = $3
            ORDER BY c.code, s.section_code
            "#,
        )
        .bind(student_id)
        .bind(actor.institution_id)
        .bind(term_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// The section a student's own enrollment belongs to — resolved before a
    /// drop so the registration screen can swap that section's row back to its
    /// available state. `None` if the enrollment is not this student's.
    pub async fn enrollment_section(
        &self,
        actor: &Actor,
        enrollment_id: Uuid,
    ) -> Result<Option<Uuid>, AppError> {
        let student_id = actor.require_student_self()?;
        Ok(sqlx::query_scalar(
            r#"
            SELECT section_id
            FROM enrollment
            WHERE id = $1 AND institution_id = $2 AND student_id = $3
            "#,
        )
        .bind(enrollment_id)
        .bind(actor.institution_id)
        .bind(student_id)
        .fetch_optional(&self.pool)
        .await?)
    }

    /// The student's own active registration holds for one term (dashboard
    /// alert). A hold blocks registration, so a non-empty list is surfaced
    /// loudly; the flags themselves are short tokens ("financial").
    pub async fn own_holds(&self, actor: &Actor, term_id: Uuid) -> Result<Vec<String>, AppError> {
        let student_id = actor.require_student_self()?;
        let flags: Option<Vec<String>> = sqlx::query_scalar(
            r#"
            SELECT hold_flags
            FROM student_term_registration
            WHERE student_id = $1 AND term_id = $2
            "#,
        )
        .bind(student_id)
        .bind(term_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(flags.unwrap_or_default())
    }

    /// A term's override records for the review list, newest first: who
    /// they cover, which rule they lift, who granted them, why, expiry, and
    /// what consumed them. The override IS this record — never a hidden
    /// boolean.
    pub async fn list_overrides(
        &self,
        actor: &Actor,
        term_id: Uuid,
    ) -> Result<Vec<OverrideRow>, AppError> {
        require_can_grant_override(actor)?;
        Ok(sqlx::query_as::<_, OverrideRow>(
            r#"
            SELECT sp.student_number,
                   ro.override_type,
                   COALESCE(c.code || ' ' || s.section_code, '') AS section_label,
                   ua.username AS granted_by,
                   COALESCE(ro.note, '') AS note,
                   ro.created_at,
                   ro.expires_at,
                   ro.consumed_at
            FROM registration_override ro
            JOIN student_profile sp ON sp.id = ro.student_id
            JOIN user_account ua ON ua.id = ro.granted_by_user_id
            LEFT JOIN section s ON s.id = ro.section_id
            LEFT JOIN course c ON c.id = s.course_id
            WHERE ro.institution_id = $1 AND ro.term_id = $2
            ORDER BY ro.created_at DESC
            LIMIT 200
            "#,
        )
        .bind(actor.institution_id)
        .bind(term_id)
        .fetch_all(&self.pool)
        .await?)
    }

    /// Search students by number or username for the registrar's lookup.
    /// Registrar-only; scoped to the institution.
    pub async fn search_students(
        &self,
        actor: &Actor,
        query: &str,
    ) -> Result<Vec<StudentHit>, AppError> {
        require_can_manage_holds(actor)?;
        let pattern = format!("%{}%", query.trim().replace('%', "\\%").replace('_', "\\_"));
        Ok(sqlx::query_as::<_, StudentHit>(
            r#"
            SELECT sp.id AS student_id, sp.student_number, sp.program_code,
                   sp.academic_status, ua.username
            FROM student_profile sp
            JOIN user_account ua ON ua.id = sp.user_id
            WHERE sp.institution_id = $1
              AND (sp.student_number ILIKE $2 OR ua.username ILIKE $2)
            ORDER BY sp.student_number
            LIMIT 50
            "#,
        )
        .bind(actor.institution_id)
        .bind(pattern)
        .fetch_all(&self.pool)
        .await?)
    }

    /// One student for the registrar's detail page; `None` (→404) outside
    /// the institution.
    pub async fn student_admin(
        &self,
        actor: &Actor,
        student_id: Uuid,
    ) -> Result<Option<StudentHit>, AppError> {
        require_can_manage_holds(actor)?;
        Ok(sqlx::query_as::<_, StudentHit>(
            r#"
            SELECT sp.id AS student_id, sp.student_number, sp.program_code,
                   sp.academic_status, ua.username
            FROM student_profile sp
            JOIN user_account ua ON ua.id = sp.user_id
            WHERE sp.institution_id = $1 AND sp.id = $2
            "#,
        )
        .bind(actor.institution_id)
        .bind(student_id)
        .fetch_optional(&self.pool)
        .await?)
    }

    /// A student's hold flags for one term, as the registrar sees them.
    pub async fn student_holds(
        &self,
        actor: &Actor,
        student_id: Uuid,
        term_id: Uuid,
    ) -> Result<Vec<String>, AppError> {
        require_can_manage_holds(actor)?;
        let flags: Option<Vec<String>> = sqlx::query_scalar(
            r#"
            SELECT str.hold_flags
            FROM student_term_registration str
            JOIN student_profile sp ON sp.id = str.student_id AND sp.institution_id = $3
            WHERE str.student_id = $1 AND str.term_id = $2
            "#,
        )
        .bind(student_id)
        .bind(term_id)
        .bind(actor.institution_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(flags.unwrap_or_default())
    }

    /// Set a student's academic standing. The status is the institution's
    /// recorded designation (it appears on transcripts); it does not itself
    /// block registration — holds do (assumption A31), and the page says so.
    pub async fn set_academic_status(
        &self,
        actor: &Actor,
        student_id: Uuid,
        status: &str,
    ) -> Result<(), AppError> {
        require_can_manage_holds(actor)?;
        const STATUSES: [&str; 5] = [
            "good_standing",
            "probation",
            "suspended",
            "withdrawn",
            "graduated",
        ];
        if !STATUSES.contains(&status) {
            return Err(AppError::Validation(
                "academic status must be one of: good_standing, probation, suspended, withdrawn, graduated"
                    .into(),
            ));
        }

        let mut tx = self.pool.begin().await?;
        let updated = sqlx::query(
            "UPDATE student_profile SET academic_status = $3 \
             WHERE id = $1 AND institution_id = $2",
        )
        .bind(student_id)
        .bind(actor.institution_id)
        .bind(status)
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
                "enrollment.academic_status_set",
                "student_profile",
                student_id,
                &serde_json::json!({ "academic_status": status }),
            )
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Active holds for a term, counted per flag — the registrar overview
    /// tile. Registrar-only, like every other cross-student hold read.
    pub async fn term_hold_counts(
        &self,
        actor: &Actor,
        term_id: Uuid,
    ) -> Result<Vec<(String, i64)>, AppError> {
        crate::enrollment::policy::require_can_manage_holds(actor)?;
        let counts: Vec<(String, i64)> = sqlx::query_as(
            r#"
            SELECT flag, count(*) AS students
            FROM student_term_registration str
            JOIN academic_term t ON t.id = str.term_id AND t.institution_id = $1,
                 unnest(str.hold_flags) AS flag
            WHERE str.term_id = $2
            GROUP BY flag
            ORDER BY students DESC, flag
            "#,
        )
        .bind(actor.institution_id)
        .bind(term_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(counts)
    }

    async fn check_hold_target(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        actor: &Actor,
        student_id: Uuid,
        term_id: Uuid,
    ) -> Result<(), AppError> {
        let both_ok: bool = sqlx::query_scalar(
            r#"
            SELECT EXISTS (SELECT 1 FROM student_profile WHERE id = $1 AND institution_id = $3)
               AND EXISTS (SELECT 1 FROM academic_term WHERE id = $2 AND institution_id = $3)
            "#,
        )
        .bind(student_id)
        .bind(term_id)
        .bind(actor.institution_id)
        .fetch_one(&mut **tx)
        .await?;
        if !both_ok {
            return Err(AppError::NotFound);
        }
        Ok(())
    }
}

/// Hold flags are short lowercase tokens ("financial", "advising"); they end
/// up in URLs and audit detail, so keep them boring.
fn valid_hold_flag(flag: &str) -> Result<&str, AppError> {
    let flag = flag.trim();
    if flag.is_empty()
        || flag.len() > 40
        || !flag
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        return Err(AppError::Validation(
            "hold flag must be a short lowercase token".into(),
        ));
    }
    Ok(flag)
}

/// One override record in the registrar's review list.
#[derive(Debug, sqlx::FromRow)]
pub struct OverrideRow {
    pub student_number: String,
    pub override_type: String,
    /// "CMPS 2131 01", or empty when the override covers the whole term.
    pub section_label: String,
    pub granted_by: String,
    pub note: String,
    pub created_at: chrono::DateTime<Utc>,
    pub expires_at: Option<chrono::DateTime<Utc>>,
    pub consumed_at: Option<chrono::DateTime<Utc>>,
}

/// One student in the registrar's lookup and detail views.
#[derive(Debug, sqlx::FromRow)]
pub struct StudentHit {
    pub student_id: Uuid,
    pub student_number: String,
    pub program_code: String,
    pub academic_status: String,
    pub username: String,
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
