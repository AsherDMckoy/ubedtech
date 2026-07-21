#[sqlx::test(migrations = "./migrations")]
async fn only_one_student_gets_the_last_seat(pool: sqlx::PgPool) {
    // Arrange one section with capacity=1 and two eligible students.
    let fixture = seed_registration_fixture(&pool, 1).await;

    let service =
        crate::enrollment::EnrollmentService::new(pool.clone(), crate::audit::AuditWriter);

    let left = service.register_for(
        &fixture.registrar,
        fixture.student_a,
        crate::enrollment::RegisterCommand {
            section_id: fixture.section_id,
            idempotency_key: uuid::Uuid::new_v4(),
        },
    );

    let right = service.register_for(
        &fixture.registrar,
        fixture.student_b,
        crate::enrollment::RegisterCommand {
            section_id: fixture.section_id,
            idempotency_key: uuid::Uuid::new_v4(),
        },
    );

    let (left, right) = tokio::join!(left, right);
    let successes = usize::from(left.is_ok()) + usize::from(right.is_ok());
    assert_eq!(successes, 1);

    let enrolled_count: i32 =
        sqlx::query_scalar("SELECT enrolled_count FROM section_capacity WHERE section_id = $1")
            .bind(fixture.section_id)
            .fetch_one(&pool)
            .await
            .unwrap();

    assert_eq!(enrolled_count, 1);
}

/// CLAUDE.md §1 item 4: a genuinely full section and a section whose capacity
/// row is missing must fail differently. Full is an ordinary business denial
/// (Conflict, "section is full"); a missing capacity row is a broken database
/// invariant (Integrity, generic 500 to the client, loud in the log).
#[sqlx::test(migrations = "./migrations")]
async fn missing_capacity_row_fails_distinctly_from_a_full_section(pool: sqlx::PgPool) {
    let fixture = seed_registration_fixture(&pool, 0).await;
    let service =
        crate::enrollment::EnrollmentService::new(pool.clone(), crate::audit::AuditWriter);

    // Capacity 0: an honest business conflict.
    let full = service
        .register_for(
            &fixture.registrar,
            fixture.student_a,
            crate::enrollment::RegisterCommand {
                section_id: fixture.section_id,
                idempotency_key: uuid::Uuid::new_v4(),
            },
        )
        .await;
    assert!(
        matches!(
            &full,
            Err(crate::enrollment::types::EnrollError::Denied(
                crate::enrollment::types::Denial::SectionFull
            ))
        ),
        "a full section is an ordinary denial: {full:?}"
    );

    // Delete the capacity row out from under the section (simulating the
    // pre-0010 defect): the same request must now fail as a broken invariant,
    // not masquerade as a full section.
    sqlx::query("DELETE FROM section_capacity WHERE section_id = $1")
        .bind(fixture.section_id)
        .execute(&pool)
        .await
        .unwrap();

    let broken = service
        .register_for(
            &fixture.registrar,
            fixture.student_a,
            crate::enrollment::RegisterCommand {
                section_id: fixture.section_id,
                idempotency_key: uuid::Uuid::new_v4(),
            },
        )
        .await;
    assert!(
        matches!(
            &broken,
            Err(crate::enrollment::types::EnrollError::App(
                crate::shared::error::AppError::Integrity(message)
            )) if message.contains("capacity")
        ),
        "a missing capacity row is an Integrity fault: {broken:?}"
    );

    // Nothing was enrolled by either failure.
    let enrollments: i64 = sqlx::query_scalar("SELECT count(*) FROM enrollment")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(enrollments, 0);
}

/// Migration 0010's trigger: inserting a bare section — any path, not just
/// the academics service — creates its capacity row in the same transaction,
/// starting at capacity 0 (fail closed until a registrar opens seats).
#[sqlx::test(migrations = "./migrations")]
async fn every_section_gets_a_capacity_row_from_the_trigger(pool: sqlx::PgPool) {
    let fixture = seed_registration_fixture(&pool, 1).await;

    let bare_section_id = uuid::Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO section (id, institution_id, term_id, course_id, section_code, status)
        SELECT $1, institution_id, term_id, course_id, '99', 'open'
        FROM section WHERE id = $2
        "#,
    )
    .bind(bare_section_id)
    .bind(fixture.section_id)
    .execute(&pool)
    .await
    .unwrap();

    let (capacity, enrolled_count): (i32, i32) = sqlx::query_as(
        "SELECT capacity, enrolled_count FROM section_capacity WHERE section_id = $1",
    )
    .bind(bare_section_id)
    .fetch_one(&pool)
    .await
    .expect("trigger created the capacity row with the section");

    assert_eq!(capacity, 0);
    assert_eq!(enrolled_count, 0);
}

/// CLAUDE.md §1 item 6: the capacity override is real, single-use, and fully
/// recorded — it admits exactly one student past a full section by raising
/// capacity and enrolled_count together, and the row is stamped with the
/// enrollment that consumed it.
#[sqlx::test(migrations = "./migrations")]
async fn capacity_override_admits_one_student_and_is_consumed_once(pool: sqlx::PgPool) {
    let fixture = seed_registration_fixture(&pool, 1).await;
    let service =
        crate::enrollment::EnrollmentService::new(pool.clone(), crate::audit::AuditWriter);
    let register = |student: uuid::Uuid| {
        service.register_for(
            &fixture.registrar,
            student,
            crate::enrollment::RegisterCommand {
                section_id: fixture.section_id,
                idempotency_key: uuid::Uuid::new_v4(),
            },
        )
    };

    register(fixture.student_a).await.expect("fills the seat");

    // Full section denies student_b until the registrar grants an override.
    assert!(matches!(
        register(fixture.student_b).await,
        Err(crate::enrollment::types::EnrollError::Denied(
            crate::enrollment::types::Denial::SectionFull
        ))
    ));

    let override_id = service
        .grant_override(
            &fixture.registrar,
            fixture.student_b,
            crate::enrollment::GrantOverrideCommand {
                term_id: fixture.term_id,
                section_id: Some(fixture.section_id),
                override_type: "capacity".into(),
                reason: "graduating senior needs this section".into(),
                expires_at: None,
            },
        )
        .await
        .unwrap();

    let receipt = register(fixture.student_b)
        .await
        .expect("override admits the student");

    // Capacity and enrolled_count moved together: the constraint still holds
    // and no free seat opened up for anyone else.
    let (capacity, enrolled): (i32, i32) = sqlx::query_as(
        "SELECT capacity, enrolled_count FROM section_capacity WHERE section_id = $1",
    )
    .bind(fixture.section_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((capacity, enrolled), (2, 2));

    // The override records which enrollment consumed it.
    let consumed_by: Option<uuid::Uuid> = sqlx::query_scalar(
        "SELECT consumed_by_enrollment_id FROM registration_override \
         WHERE id = $1 AND consumed_at IS NOT NULL",
    )
    .bind(override_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(consumed_by, Some(receipt.enrollment_id));

    // Both audit trails name the override.
    let grant_audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_event \
         WHERE action = 'enrollment.override_granted' AND resource_id = $1",
    )
    .bind(override_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(grant_audits, 1);
    let registration_names_override: bool = sqlx::query_scalar(
        "SELECT detail->'overrides_consumed' @> to_jsonb(ARRAY[$1::uuid]) FROM audit_event \
         WHERE action = 'enrollment.registered' AND resource_id = $2",
    )
    .bind(override_id)
    .bind(receipt.enrollment_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(registration_names_override);

    // Consumed means consumed: after dropping, the same override does not
    // admit a second registration into the (again full) section.
    service
        .drop_for(&fixture.registrar, fixture.student_b, receipt.enrollment_id)
        .await
        .unwrap();
    assert!(matches!(
        register(fixture.student_b).await,
        Err(crate::enrollment::types::EnrollError::Denied(
            crate::enrollment::types::Denial::SectionFull
        ))
    ));
}

/// A registrar `deadline` override admits one late add (and a second one a
/// late drop); without one the window stays shut. It lifts only the closing
/// deadline — registration that has not opened yet stays closed.
#[sqlx::test(migrations = "./migrations")]
async fn deadline_override_admits_a_late_add_and_a_late_drop(pool: sqlx::PgPool) {
    let fixture = seed_registration_fixture(&pool, 2).await;
    let service =
        crate::enrollment::EnrollmentService::new(pool.clone(), crate::audit::AuditWriter);
    let grant_deadline_override = |student: uuid::Uuid| {
        service.grant_override(
            &fixture.registrar,
            student,
            crate::enrollment::GrantOverrideCommand {
                term_id: fixture.term_id,
                section_id: None,
                override_type: "deadline".into(),
                reason: "medical exception approved by the dean".into(),
                expires_at: None,
            },
        )
    };

    sqlx::query("UPDATE academic_term SET add_drop_closes_at = now() - interval '1 minute'")
        .execute(&pool)
        .await
        .unwrap();

    grant_deadline_override(fixture.student_a).await.unwrap();
    let receipt = service
        .register_for(
            &fixture.registrar,
            fixture.student_a,
            crate::enrollment::RegisterCommand {
                section_id: fixture.section_id,
                idempotency_key: uuid::Uuid::new_v4(),
            },
        )
        .await
        .expect("deadline override admits the late add");

    // The override was consumed by the add; the late drop needs its own.
    assert!(matches!(
        service
            .drop_for(&fixture.registrar, fixture.student_a, receipt.enrollment_id)
            .await,
        Err(crate::enrollment::types::EnrollError::Denied(
            crate::enrollment::types::Denial::WindowClosed
        ))
    ));

    grant_deadline_override(fixture.student_a).await.unwrap();
    service
        .drop_for(&fixture.registrar, fixture.student_a, receipt.enrollment_id)
        .await
        .expect("second deadline override admits the late drop");

    // An override never opens registration early.
    sqlx::query(
        "UPDATE academic_term SET registration_opens_at = now() + interval '1 hour', \
         add_drop_closes_at = now() + interval '1 day'",
    )
    .execute(&pool)
    .await
    .unwrap();
    grant_deadline_override(fixture.student_b).await.unwrap();
    assert!(matches!(
        service
            .register_for(
                &fixture.registrar,
                fixture.student_b,
                crate::enrollment::RegisterCommand {
                    section_id: fixture.section_id,
                    idempotency_key: uuid::Uuid::new_v4(),
                },
            )
            .await,
        Err(crate::enrollment::types::EnrollError::Denied(
            crate::enrollment::types::Denial::WindowClosed
        ))
    ));
}

/// Override grants are registrar-only, validated, and institution-scoped.
#[sqlx::test(migrations = "./migrations")]
async fn override_grants_are_registrar_only_validated_and_scoped(pool: sqlx::PgPool) {
    use crate::shared::error::AppError;

    let fixture = seed_registration_fixture(&pool, 1).await;
    let other = seed_registration_fixture(&pool, 1).await;
    let service =
        crate::enrollment::EnrollmentService::new(pool.clone(), crate::audit::AuditWriter);
    let command = |override_type: &str, reason: &str| crate::enrollment::GrantOverrideCommand {
        term_id: fixture.term_id,
        section_id: None,
        override_type: override_type.into(),
        reason: reason.into(),
        expires_at: None,
    };

    // A student (even acting on themselves) cannot grant overrides.
    let student_actor = crate::shared::actor::Actor {
        user_id: fixture.registrar.user_id,
        institution_id: fixture.registrar.institution_id,
        student_id: Some(fixture.student_a),
        roles: std::collections::HashSet::from([crate::shared::actor::Role::Student]),
    };
    assert!(matches!(
        service
            .grant_override(&student_actor, fixture.student_a, command("hold", "please"))
            .await,
        Err(AppError::Forbidden)
    ));

    // Unknown rule and blank reason are validation errors.
    assert!(matches!(
        service
            .grant_override(
                &fixture.registrar,
                fixture.student_a,
                command("gpa", "reason")
            )
            .await,
        Err(AppError::Validation(_))
    ));
    assert!(matches!(
        service
            .grant_override(&fixture.registrar, fixture.student_a, command("hold", "  "))
            .await,
        Err(AppError::Validation(_))
    ));

    // Expiry must be in the future.
    let mut expired = command("hold", "reason");
    expired.expires_at = Some(chrono::Utc::now() - chrono::Duration::minutes(1));
    assert!(matches!(
        service
            .grant_override(&fixture.registrar, fixture.student_a, expired)
            .await,
        Err(AppError::Validation(_))
    ));

    // Another institution's student (or term) answers 404, and no override
    // row or audit is written for any of the failures above.
    assert!(matches!(
        service
            .grant_override(
                &fixture.registrar,
                other.student_a,
                command("hold", "reason")
            )
            .await,
        Err(AppError::NotFound)
    ));
    let overrides: i64 = sqlx::query_scalar("SELECT count(*) FROM registration_override")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(overrides, 0);
}

/// Holds: a registrar-placed hold blocks registration with its own distinct
/// denial; releasing it (or a single-use hold override) admits the student.
/// Placement/release are registrar-only, institution-scoped, and idempotent.
#[sqlx::test(migrations = "./migrations")]
async fn holds_block_registration_until_released_or_overridden(pool: sqlx::PgPool) {
    use crate::shared::error::AppError;

    let fixture = seed_registration_fixture(&pool, 5).await;
    let other = seed_registration_fixture(&pool, 5).await;
    let service =
        crate::enrollment::EnrollmentService::new(pool.clone(), crate::audit::AuditWriter);
    let register = |student: uuid::Uuid| {
        service.register_for(
            &fixture.registrar,
            student,
            crate::enrollment::RegisterCommand {
                section_id: fixture.section_id,
                idempotency_key: uuid::Uuid::new_v4(),
            },
        )
    };

    // Only the registrar places holds; cross-institution targets 404; junk
    // flags are rejected.
    let student_actor = crate::shared::actor::Actor {
        user_id: fixture.registrar.user_id,
        institution_id: fixture.registrar.institution_id,
        student_id: Some(fixture.student_a),
        roles: std::collections::HashSet::from([crate::shared::actor::Role::Student]),
    };
    assert!(matches!(
        service
            .place_hold(
                &student_actor,
                fixture.student_a,
                fixture.term_id,
                "financial",
                "self-inflicted"
            )
            .await,
        Err(AppError::Forbidden)
    ));
    assert!(matches!(
        service
            .place_hold(
                &fixture.registrar,
                other.student_a,
                fixture.term_id,
                "financial",
                "wrong school"
            )
            .await,
        Err(AppError::NotFound)
    ));
    assert!(matches!(
        service
            .place_hold(
                &fixture.registrar,
                fixture.student_a,
                fixture.term_id,
                "Fin ancial!",
                "reason"
            )
            .await,
        Err(AppError::Validation(_))
    ));

    // Hold placed: registration is denied with the hold-specific reason.
    service
        .place_hold(
            &fixture.registrar,
            fixture.student_a,
            fixture.term_id,
            "financial",
            "unpaid balance",
        )
        .await
        .unwrap();
    assert!(matches!(
        register(fixture.student_a).await,
        Err(crate::enrollment::types::EnrollError::Denied(
            crate::enrollment::types::Denial::Hold
        ))
    ));

    // Idempotent re-place: no second audit row.
    service
        .place_hold(
            &fixture.registrar,
            fixture.student_a,
            fixture.term_id,
            "financial",
            "unpaid balance",
        )
        .await
        .unwrap();
    let hold_audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_event WHERE action = 'enrollment.hold_placed'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(hold_audits, 1);

    // A hold override admits one registration while the hold stands.
    service
        .grant_override(
            &fixture.registrar,
            fixture.student_a,
            crate::enrollment::GrantOverrideCommand {
                term_id: fixture.term_id,
                section_id: Some(fixture.section_id),
                override_type: "hold".into(),
                reason: "dean approved despite balance".into(),
                expires_at: None,
            },
        )
        .await
        .unwrap();
    register(fixture.student_a)
        .await
        .expect("hold override admits");

    // Releasing a hold unblocks the student entirely.
    service
        .place_hold(
            &fixture.registrar,
            fixture.student_b,
            fixture.term_id,
            "financial",
            "unpaid balance",
        )
        .await
        .unwrap();
    assert!(matches!(
        register(fixture.student_b).await,
        Err(crate::enrollment::types::EnrollError::Denied(
            crate::enrollment::types::Denial::Hold
        ))
    ));
    service
        .release_hold(
            &fixture.registrar,
            fixture.student_b,
            fixture.term_id,
            "financial",
        )
        .await
        .unwrap();
    register(fixture.student_b)
        .await
        .expect("released hold no longer blocks");
}

/// ADR-8: one shared `add_drop_closes_at` governs adds AND drops. Before the
/// deadline both actions work; after it both are refused — there is no window
/// where a student may drop but not add (or vice versa).
#[sqlx::test(migrations = "./migrations")]
async fn one_deadline_governs_both_adds_and_drops(pool: sqlx::PgPool) {
    let fixture = seed_registration_fixture(&pool, 2).await;
    let service =
        crate::enrollment::EnrollmentService::new(pool.clone(), crate::audit::AuditWriter);

    // Inside the window: an add succeeds.
    let receipt = service
        .register_for(
            &fixture.registrar,
            fixture.student_a,
            crate::enrollment::RegisterCommand {
                section_id: fixture.section_id,
                idempotency_key: uuid::Uuid::new_v4(),
            },
        )
        .await
        .expect("add inside the window succeeds");

    // Past the deadline: adds and drops are both refused.
    sqlx::query(
        "UPDATE academic_term SET add_drop_closes_at = now() - interval '1 minute' \
         WHERE institution_id = $1",
    )
    .bind(fixture.registrar.institution_id)
    .execute(&pool)
    .await
    .unwrap();

    let late_add = service
        .register_for(
            &fixture.registrar,
            fixture.student_b,
            crate::enrollment::RegisterCommand {
                section_id: fixture.section_id,
                idempotency_key: uuid::Uuid::new_v4(),
            },
        )
        .await;
    assert!(
        matches!(
            &late_add,
            Err(crate::enrollment::types::EnrollError::Denied(
                crate::enrollment::types::Denial::WindowClosed
            ))
        ),
        "late add must be refused: {late_add:?}"
    );

    let late_drop = service
        .drop_for(&fixture.registrar, fixture.student_a, receipt.enrollment_id)
        .await;
    assert!(
        matches!(
            &late_drop,
            Err(crate::enrollment::types::EnrollError::Denied(
                crate::enrollment::types::Denial::WindowClosed
            ))
        ),
        "late drop must be refused: {late_drop:?}"
    );

    // Reopen the window: the drop now works and frees the seat.
    sqlx::query(
        "UPDATE academic_term SET add_drop_closes_at = now() + interval '1 day' \
         WHERE institution_id = $1",
    )
    .bind(fixture.registrar.institution_id)
    .execute(&pool)
    .await
    .unwrap();

    service
        .drop_for(&fixture.registrar, fixture.student_a, receipt.enrollment_id)
        .await
        .expect("drop inside the reopened window succeeds");

    let enrolled_count: i32 =
        sqlx::query_scalar("SELECT enrolled_count FROM section_capacity WHERE section_id = $1")
            .bind(fixture.section_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(enrolled_count, 0);
}

/// The required idempotency proof: resubmitting the same idempotency key —
/// sequentially or concurrently — returns the ORIGINAL receipt, never a
/// duplicate enrollment and never an error.
#[sqlx::test(migrations = "./migrations")]
async fn idempotent_resubmission_returns_the_original_receipt(pool: sqlx::PgPool) {
    let fixture = seed_registration_fixture(&pool, 10).await;
    let service =
        crate::enrollment::EnrollmentService::new(pool.clone(), crate::audit::AuditWriter);
    let key = uuid::Uuid::new_v4();
    let register = || {
        service.register_for(
            &fixture.registrar,
            fixture.student_a,
            crate::enrollment::RegisterCommand {
                section_id: fixture.section_id,
                idempotency_key: key,
            },
        )
    };

    // Two concurrent submissions of the same key: both succeed, same receipt.
    let (left, right) = tokio::join!(register(), register());
    let left = left.expect("concurrent resubmission is not an error");
    let right = right.expect("concurrent resubmission is not an error");
    assert_eq!(left.enrollment_id, right.enrollment_id);

    // A later retry (the classic browser re-POST) gets the same receipt too.
    let retry = register().await.unwrap();
    assert_eq!(retry.enrollment_id, left.enrollment_id);
    assert_eq!(retry.registered_at, left.registered_at);

    // Exactly one enrollment, one reserved seat, one audit row came out of
    // three submissions.
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM enrollment")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rows, 1);
    let enrolled: i32 =
        sqlx::query_scalar("SELECT enrolled_count FROM section_capacity WHERE section_id = $1")
            .bind(fixture.section_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(enrolled, 1);
    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_event WHERE action = 'enrollment.registered'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(audits, 1);
}

/// A different idempotency key does not smuggle a student into the same
/// section twice.
#[sqlx::test(migrations = "./migrations")]
async fn duplicate_enrollment_in_the_same_section_is_denied(pool: sqlx::PgPool) {
    let fixture = seed_registration_fixture(&pool, 10).await;
    let service =
        crate::enrollment::EnrollmentService::new(pool.clone(), crate::audit::AuditWriter);
    let register = || {
        service.register_for(
            &fixture.registrar,
            fixture.student_a,
            crate::enrollment::RegisterCommand {
                section_id: fixture.section_id,
                idempotency_key: uuid::Uuid::new_v4(),
            },
        )
    };

    register().await.unwrap();
    assert!(matches!(
        register().await,
        Err(crate::enrollment::types::EnrollError::Denied(
            crate::enrollment::types::Denial::AlreadyEnrolled
        ))
    ));
    let enrolled: i32 =
        sqlx::query_scalar("SELECT enrolled_count FROM section_capacity WHERE section_id = $1")
            .bind(fixture.section_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(enrolled, 1);
}

/// Overlapping meeting times in the same term are a denial; back-to-back
/// times are not.
#[sqlx::test(migrations = "./migrations")]
async fn schedule_conflicts_are_detected_by_meeting_overlap(pool: sqlx::PgPool) {
    let fixture = seed_registration_fixture(&pool, 10).await;
    let service =
        crate::enrollment::EnrollmentService::new(pool.clone(), crate::audit::AuditWriter);
    let register = |section: uuid::Uuid| {
        service.register_for(
            &fixture.registrar,
            fixture.student_a,
            crate::enrollment::RegisterCommand {
                section_id: section,
                idempotency_key: uuid::Uuid::new_v4(),
            },
        )
    };

    add_meeting(&pool, fixture.section_id, 1, "09:00:00", "10:00:00").await;
    let overlapping = add_course_section(&pool, &fixture, "OVERLAP", 10).await;
    add_meeting(&pool, overlapping, 1, "09:30:00", "10:30:00").await;
    let back_to_back = add_course_section(&pool, &fixture, "SEQUEL", 10).await;
    add_meeting(&pool, back_to_back, 1, "10:00:00", "11:00:00").await;

    register(fixture.section_id).await.unwrap();
    assert!(matches!(
        register(overlapping).await,
        Err(crate::enrollment::types::EnrollError::Denied(
            crate::enrollment::types::Denial::ScheduleConflict
        ))
    ));
    register(back_to_back)
        .await
        .expect("adjacent meeting times are not a conflict");
}

/// Prerequisites: an unmet or under-grade prerequisite denies; a published
/// completion at or above the minimum admits.
#[sqlx::test(migrations = "./migrations")]
async fn prerequisites_deny_until_completed_with_the_minimum_grade(pool: sqlx::PgPool) {
    let fixture = seed_registration_fixture(&pool, 10).await;
    let service =
        crate::enrollment::EnrollmentService::new(pool.clone(), crate::audit::AuditWriter);
    let register = || {
        service.register_for(
            &fixture.registrar,
            fixture.student_a,
            crate::enrollment::RegisterCommand {
                section_id: fixture.section_id,
                idempotency_key: uuid::Uuid::new_v4(),
            },
        )
    };

    // The fixture's course now requires PREREQ with at least 2.0 points.
    let prereq_section = add_course_section(&pool, &fixture, "PREREQ", 10).await;
    let prereq_course: uuid::Uuid =
        sqlx::query_scalar("SELECT course_id FROM section WHERE id = $1")
            .bind(prereq_section)
            .fetch_one(&pool)
            .await
            .unwrap();
    let target_course: uuid::Uuid =
        sqlx::query_scalar("SELECT course_id FROM section WHERE id = $1")
            .bind(fixture.section_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    sqlx::query(
        "INSERT INTO course_prerequisite (course_id, prerequisite_course_id, \
         minimum_grade_points) VALUES ($1, $2, 2.0)",
    )
    .bind(target_course)
    .bind(prereq_course)
    .execute(&pool)
    .await
    .unwrap();

    // No completion at all: denied.
    assert!(matches!(
        register().await,
        Err(crate::enrollment::types::EnrollError::Denied(
            crate::enrollment::types::Denial::PrerequisiteNotMet
        ))
    ));

    // A published grade below the minimum still denies.
    let prereq_enrollment =
        seed_completion(&pool, &fixture, prereq_section, fixture.student_a, 1.0).await;
    assert!(matches!(
        register().await,
        Err(crate::enrollment::types::EnrollError::Denied(
            crate::enrollment::types::Denial::PrerequisiteNotMet
        ))
    ));

    // Raising it to the minimum admits.
    sqlx::query("UPDATE grade_record SET grade_points = 3.0 WHERE enrollment_id = $1")
        .bind(prereq_enrollment)
        .execute(&pool)
        .await
        .unwrap();
    register().await.expect("satisfied prerequisite admits");
}

/// Two concurrent drops of the same enrollment release exactly one seat.
#[sqlx::test(migrations = "./migrations")]
async fn concurrent_duplicate_drops_release_exactly_one_seat(pool: sqlx::PgPool) {
    let fixture = seed_registration_fixture(&pool, 10).await;
    let service =
        crate::enrollment::EnrollmentService::new(pool.clone(), crate::audit::AuditWriter);

    let receipt = service
        .register_for(
            &fixture.registrar,
            fixture.student_a,
            crate::enrollment::RegisterCommand {
                section_id: fixture.section_id,
                idempotency_key: uuid::Uuid::new_v4(),
            },
        )
        .await
        .unwrap();

    let left = service.drop_for(&fixture.registrar, fixture.student_a, receipt.enrollment_id);
    let right = service.drop_for(&fixture.registrar, fixture.student_a, receipt.enrollment_id);
    let (left, right) = tokio::join!(left, right);
    assert_eq!(
        usize::from(left.is_ok()) + usize::from(right.is_ok()),
        1,
        "exactly one drop succeeds: {left:?} / {right:?}"
    );

    let enrolled: i32 =
        sqlx::query_scalar("SELECT enrolled_count FROM section_capacity WHERE section_id = $1")
            .bind(fixture.section_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(enrolled, 0, "the seat was released exactly once");
}

/// A drop racing a registration for the last seat never oversells and never
/// loses a seat: the counter always equals the surviving active enrollments.
#[sqlx::test(migrations = "./migrations")]
async fn a_drop_racing_a_registration_keeps_the_counter_honest(pool: sqlx::PgPool) {
    let fixture = seed_registration_fixture(&pool, 1).await;
    let service =
        crate::enrollment::EnrollmentService::new(pool.clone(), crate::audit::AuditWriter);

    let receipt = service
        .register_for(
            &fixture.registrar,
            fixture.student_a,
            crate::enrollment::RegisterCommand {
                section_id: fixture.section_id,
                idempotency_key: uuid::Uuid::new_v4(),
            },
        )
        .await
        .unwrap();

    let drop = service.drop_for(&fixture.registrar, fixture.student_a, receipt.enrollment_id);
    let add = service.register_for(
        &fixture.registrar,
        fixture.student_b,
        crate::enrollment::RegisterCommand {
            section_id: fixture.section_id,
            idempotency_key: uuid::Uuid::new_v4(),
        },
    );
    let (drop, add) = tokio::join!(drop, add);
    drop.expect("the drop always succeeds");
    // The add may win the freed seat or find the section still full —
    // both are correct; overselling is not.

    let active: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM enrollment WHERE section_id = $1 AND status = 'enrolled'",
    )
    .bind(fixture.section_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let enrolled_count: i32 =
        sqlx::query_scalar("SELECT enrolled_count FROM section_capacity WHERE section_id = $1")
            .bind(fixture.section_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(i64::from(enrolled_count), active);
    assert_eq!(active == 1, add.is_ok());
    assert!(enrolled_count <= 1);
}

/// New open section of a new course in the fixture's institution and term.
async fn add_course_section(
    pool: &sqlx::PgPool,
    fixture: &Fixture,
    course_code: &str,
    capacity: i32,
) -> uuid::Uuid {
    let course_id = uuid::Uuid::new_v4();
    let section_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO course (id, institution_id, code, title, credit_hours) \
         VALUES ($1, $2, $3, $3, 3.0)",
    )
    .bind(course_id)
    .bind(fixture.registrar.institution_id)
    .bind(course_code)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO section (id, institution_id, term_id, course_id, section_code, status) \
         VALUES ($1, $2, $3, $4, '01', 'open')",
    )
    .bind(section_id)
    .bind(fixture.registrar.institution_id)
    .bind(fixture.term_id)
    .bind(course_id)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("UPDATE section_capacity SET capacity = $2 WHERE section_id = $1")
        .bind(section_id)
        .bind(capacity)
        .execute(pool)
        .await
        .unwrap();
    section_id
}

async fn add_meeting(
    pool: &sqlx::PgPool,
    section_id: uuid::Uuid,
    day: i16,
    starts: &str,
    ends: &str,
) {
    sqlx::query(
        "INSERT INTO section_meeting (id, section_id, day_of_week, starts_at, ends_at) \
         VALUES ($1, $2, $3, $4::time, $5::time)",
    )
    .bind(uuid::Uuid::new_v4())
    .bind(section_id)
    .bind(day)
    .bind(starts)
    .bind(ends)
    .execute(pool)
    .await
    .unwrap();
}

/// A finished course: enrollment + published grade with the given points.
async fn seed_completion(
    pool: &sqlx::PgPool,
    fixture: &Fixture,
    section_id: uuid::Uuid,
    student_id: uuid::Uuid,
    grade_points: f64,
) -> uuid::Uuid {
    let enrollment_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO enrollment (id, institution_id, student_id, section_id, status, \
         registered_at, source, idempotency_key, created_by_user_id) \
         VALUES ($1, $2, $3, $4, 'enrolled', now(), 'registrar', $5, $6)",
    )
    .bind(enrollment_id)
    .bind(fixture.registrar.institution_id)
    .bind(student_id)
    .bind(section_id)
    .bind(uuid::Uuid::new_v4())
    .bind(fixture.registrar.user_id)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO grade_record (id, institution_id, enrollment_id, grade_code, \
         grade_points, state, entered_by_user_id, published_at) \
         VALUES ($1, $2, $3, 'X', $4, 'published', $5, now())",
    )
    .bind(uuid::Uuid::new_v4())
    .bind(fixture.registrar.institution_id)
    .bind(enrollment_id)
    .bind(grade_points)
    .bind(fixture.registrar.user_id)
    .execute(pool)
    .await
    .unwrap();
    enrollment_id
}

async fn seed_registration_fixture(pool: &sqlx::PgPool, capacity: i32) -> Fixture {
    use crate::shared::actor::{Actor, Role};
    use chrono::{Duration, Utc};
    use std::collections::HashSet;
    use uuid::Uuid;

    let institution_id = Uuid::new_v4();
    let registrar_user_id = Uuid::new_v4();
    let student_user_a = Uuid::new_v4();
    let student_user_b = Uuid::new_v4();
    let student_a = Uuid::new_v4();
    let student_b = Uuid::new_v4();
    let term_id = Uuid::new_v4();
    let course_id = Uuid::new_v4();
    let section_id = Uuid::new_v4();

    let now = Utc::now();
    let mut tx = pool.begin().await.unwrap();

    sqlx::query("INSERT INTO institution (id, code, name) VALUES ($1, $2, 'Test University')")
        .bind(institution_id)
        .bind(format!("T-{}", &institution_id.to_string()[..8]))
        .execute(&mut *tx)
        .await
        .unwrap();

    for (id, username, email) in [
        (registrar_user_id, "registrar", "registrar@test.invalid"),
        (student_user_a, "student-a", "student-a@test.invalid"),
        (student_user_b, "student-b", "student-b@test.invalid"),
    ] {
        sqlx::query(
            r#"
            INSERT INTO user_account (id, institution_id, username, email)
            VALUES ($1, $2, $3, $4)
            "#,
        )
        .bind(id)
        .bind(institution_id)
        .bind(format!("{username}-{}", &institution_id.to_string()[..8]))
        .bind(format!("{}-{}", &institution_id.to_string()[..8], email))
        .execute(&mut *tx)
        .await
        .unwrap();
    }

    for (id, user_id, number) in [
        (student_a, student_user_a, "A-001"),
        (student_b, student_user_b, "B-001"),
    ] {
        sqlx::query(
            r#"
            INSERT INTO student_profile (
                id, institution_id, user_id, student_number, program_code
            )
            VALUES ($1, $2, $3, $4, 'TEST')
            "#,
        )
        .bind(id)
        .bind(institution_id)
        .bind(user_id)
        .bind(format!("{}-{number}", &institution_id.to_string()[..8]))
        .execute(&mut *tx)
        .await
        .unwrap();
    }

    sqlx::query(
        r#"
        INSERT INTO academic_term (
            id, institution_id, code, name, starts_on, ends_on,
            registration_opens_at, add_drop_closes_at
        )
        VALUES ($1, $2, $3, 'Test Term', $4, $5, $6, $7)
        "#,
    )
    .bind(term_id)
    .bind(institution_id)
    .bind(format!("TERM-{}", &term_id.to_string()[..8]))
    .bind(now.date_naive())
    .bind((now + Duration::days(120)).date_naive())
    .bind(now - Duration::hours(1))
    .bind(now + Duration::days(7))
    .execute(&mut *tx)
    .await
    .unwrap();

    sqlx::query(
        r#"
        INSERT INTO course (id, institution_id, code, title, credit_hours)
        VALUES ($1, $2, $3, 'Concurrency Test', 3.0)
        "#,
    )
    .bind(course_id)
    .bind(institution_id)
    .bind(format!("TEST-{}", &course_id.to_string()[..8]))
    .execute(&mut *tx)
    .await
    .unwrap();

    sqlx::query(
        r#"
        INSERT INTO section (
            id, institution_id, term_id, course_id, section_code, status
        )
        VALUES ($1, $2, $3, $4, '01', 'open')
        "#,
    )
    .bind(section_id)
    .bind(institution_id)
    .bind(term_id)
    .bind(course_id)
    .execute(&mut *tx)
    .await
    .unwrap();

    sqlx::query(
        r#"
        INSERT INTO section_capacity (section_id, capacity, enrolled_count)
        VALUES ($1, $2, 0)
        ON CONFLICT (section_id) DO UPDATE SET capacity = EXCLUDED.capacity
        "#,
    )
    .bind(section_id)
    .bind(capacity)
    .execute(&mut *tx)
    .await
    .unwrap();

    tx.commit().await.unwrap();

    Fixture {
        registrar: Actor {
            user_id: registrar_user_id,
            institution_id,
            student_id: None,
            roles: HashSet::from([Role::Registrar]),
        },
        student_a,
        student_b,
        student_user_a,
        section_id,
        term_id,
    }
}

struct Fixture {
    registrar: crate::shared::actor::Actor,
    student_a: uuid::Uuid,
    student_b: uuid::Uuid,
    student_user_a: uuid::Uuid,
    section_id: uuid::Uuid,
    term_id: uuid::Uuid,
}

// ---------------------------------------------------------------------------
// Student-facing pages: full HTTP flows over plain forms — the proof the UI
// works with JavaScript off (nothing here executes a script).
// ---------------------------------------------------------------------------

mod ui {
    use super::{Fixture, add_course_section, add_meeting, seed_registration_fixture};
    use actix_web::http::StatusCode;
    use actix_web::{test as actix_test, web};
    use sqlx::PgPool;
    use uuid::Uuid;

    use crate::identity_access::http::SessionCookiePolicy;
    use crate::identity_access::password::PasswordService;
    use crate::identity_access::service::AuthService;
    use crate::identity_access::sessions::SessionService;
    use crate::licensing::{LicenseGate, LicenseSnapshot, LicenseStatus};

    const PASSWORD: &str = "correct horse battery";

    macro_rules! ui_app {
        ($pool:expr, $institution:expr) => {{
            let sessions = SessionService::new($pool.clone(), 1800, 43200);
            let auth = AuthService::new(
                $pool.clone(),
                PasswordService::new(8, 1, 1).unwrap(),
                sessions.clone(),
                crate::audit::AuditWriter,
                10,
                900,
            )
            .unwrap();
            let gate = LicenseGate::new(LicenseSnapshot {
                institution_id: $institution,
                deployment_id: Uuid::new_v4(),
                status: LicenseStatus::Active,
                valid_from: chrono::Utc::now() - chrono::Duration::days(1),
                valid_until: chrono::Utc::now() + chrono::Duration::days(365),
                version: 1,
                feature_set: serde_json::json!({}),
            });
            actix_test::init_service(
                actix_web::App::new()
                    .app_data(web::Data::new(sessions))
                    .app_data(web::Data::new(auth))
                    .app_data(web::Data::new(gate))
                    .app_data(web::Data::new(SessionCookiePolicy {
                        secure: false,
                        max_age_secs: 43200,
                    }))
                    .app_data(web::Data::new(crate::academics::AcademicsService::new(
                        $pool.clone(),
                        crate::audit::AuditWriter,
                    )))
                    .app_data(web::Data::new(crate::enrollment::EnrollmentService::new(
                        $pool.clone(),
                        crate::audit::AuditWriter,
                    )))
                    .app_data(web::Data::new(crate::institution::InstitutionService::new(
                        $pool.clone(),
                        crate::audit::AuditWriter,
                    )))
                    // Same order as main.rs: csrf inside session resolution.
                    .wrap(actix_web::middleware::from_fn(
                        crate::identity_access::csrf::csrf_middleware,
                    ))
                    .wrap(actix_web::middleware::from_fn(
                        crate::identity_access::middleware::session_middleware,
                    ))
                    .configure(crate::identity_access::http::routes)
                    .configure(crate::academics::http::routes)
                    .configure(crate::enrollment::http::routes),
            )
            .await
        }};
    }

    /// Give the fixture's student A a login credential and the student role.
    async fn credential_student(pool: &PgPool, fixture: &Fixture) -> String {
        let username = format!("stu-{}", &fixture.student_user_a.to_string()[..8]);
        sqlx::query("UPDATE user_account SET username = $2 WHERE id = $1")
            .bind(fixture.student_user_a)
            .bind(&username)
            .execute(pool)
            .await
            .unwrap();
        let hash = PasswordService::new(8, 1, 1)
            .unwrap()
            .hash(PASSWORD)
            .unwrap();
        sqlx::query("INSERT INTO password_credential (user_id, password_hash) VALUES ($1, $2)")
            .bind(fixture.student_user_a)
            .bind(hash)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO user_role (institution_id, user_id, role_id) \
             SELECT $1, $2, id FROM role WHERE code = 'student'",
        )
        .bind(fixture.registrar.institution_id)
        .bind(fixture.student_user_a)
        .execute(pool)
        .await
        .unwrap();
        username
    }

    /// `<input ... name="{name}" ... value="...">` — first match, attribute
    /// order and whitespace agnostic.
    fn extract_input(body: &str, name: &str) -> String {
        let at = body
            .find(&format!("name=\"{name}\""))
            .unwrap_or_else(|| panic!("no input named {name} in page"));
        let rest = &body[at..];
        let value_at = rest.find("value=\"").expect("input has a value") + "value=\"".len();
        rest[value_at..]
            .split('"')
            .next()
            .expect("closing quote")
            .to_owned()
    }

    async fn login<S, B>(
        app: &S,
        username: &str,
        password: &str,
    ) -> actix_web::cookie::Cookie<'static>
    where
        S: actix_web::dev::Service<
                actix_http::Request,
                Response = actix_web::dev::ServiceResponse<B>,
                Error = actix_web::Error,
            >,
        B: actix_web::body::MessageBody,
    {
        let response = actix_test::call_service(
            app,
            actix_test::TestRequest::post()
                .uri("/ui/login")
                .peer_addr("127.0.0.1:9999".parse().unwrap())
                .set_form(serde_json::json!({ "username": username, "password": password }))
                .to_request(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            response.headers().get("Location").unwrap(),
            "/ui/registration"
        );
        response
            .response()
            .cookies()
            .find(|cookie| cookie.name() == "ub_session")
            .expect("form login sets the session cookie")
            .into_owned()
    }

    async fn get_page<S, B>(
        app: &S,
        cookie: &actix_web::cookie::Cookie<'static>,
        uri: &str,
    ) -> String
    where
        S: actix_web::dev::Service<
                actix_http::Request,
                Response = actix_web::dev::ServiceResponse<B>,
                Error = actix_web::Error,
            >,
        B: actix_web::body::MessageBody,
    {
        let response = actix_test::call_service(
            app,
            actix_test::TestRequest::get()
                .uri(uri)
                .cookie(cookie.clone())
                .to_request(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK, "GET {uri}");
        let body = String::from_utf8(actix_test::read_body(response).await.to_vec()).unwrap();
        crate::shared::assets::assert_page_a11y(&body);
        body
    }

    /// POST a registration form; returns (status, body-as-text).
    async fn post_add<S, B>(
        app: &S,
        cookie: &actix_web::cookie::Cookie<'static>,
        csrf: &str,
        section_id: Uuid,
    ) -> (StatusCode, String)
    where
        S: actix_web::dev::Service<
                actix_http::Request,
                Response = actix_web::dev::ServiceResponse<B>,
                Error = actix_web::Error,
            >,
        B: actix_web::body::MessageBody,
    {
        let response = actix_test::call_service(
            app,
            actix_test::TestRequest::post()
                .uri("/ui/registration/add")
                .cookie(cookie.clone())
                .set_form(serde_json::json!({
                    "csrf_token": csrf,
                    "section_id": section_id,
                    "idempotency_key": Uuid::new_v4(),
                }))
                .to_request(),
        )
        .await;
        let status = response.status();
        let body = String::from_utf8(actix_test::read_body(response).await.to_vec()).unwrap();
        (status, body)
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn login_catalog_register_and_drop_work_as_plain_forms(pool: PgPool) {
        let fixture = seed_registration_fixture(&pool, 1).await;
        let username = credential_student(&pool, &fixture).await;
        let app = ui_app!(&pool, fixture.registrar.institution_id);

        // The login page is reachable anonymously and is a plain form.
        let response = actix_test::call_service(
            &app,
            actix_test::TestRequest::get().uri("/ui/login").to_request(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = String::from_utf8(actix_test::read_body(response).await.to_vec()).unwrap();
        crate::shared::assets::assert_page_a11y(&body);
        assert!(body.contains("form method=\"post\" action=\"/ui/login\""));

        // A wrong password re-renders the page with the error inline (401).
        let response = actix_test::call_service(
            &app,
            actix_test::TestRequest::post()
                .uri("/ui/login")
                .peer_addr("127.0.0.1:9999".parse().unwrap())
                .set_form(serde_json::json!({ "username": &username, "password": "wrong" }))
                .to_request(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let body = String::from_utf8(actix_test::read_body(response).await.to_vec()).unwrap();
        assert!(body.contains("Invalid username or password."));

        // Real login → the dashboard renders with the term and campus events
        // strip, and the a11y structure holds.
        let cookie = login(&app, &username, PASSWORD).await;
        let dashboard = get_page(&app, &cookie, "/ui/dashboard").await;
        crate::shared::assets::assert_page_a11y(&dashboard);
        assert!(dashboard.contains("Your dashboard"));
        assert!(dashboard.contains("Add/drop closes"));

        // Catalog shows the section with seats and a register form.
        let catalog = get_page(&app, &cookie, "/ui/catalog").await;
        assert!(catalog.contains("Concurrency Test"), "course listed");
        assert!(catalog.contains("0/1"), "seat counts shown");
        let csrf = extract_input(&catalog, "csrf_token");
        let listed_section: Uuid = extract_input(&catalog, "section_id").parse().unwrap();
        assert_eq!(listed_section, fixture.section_id);

        // Register via the form → PRG redirect → panel shows the enrollment.
        let (status, _) = post_add(&app, &cookie, &csrf, fixture.section_id).await;
        assert_eq!(status, StatusCode::SEE_OTHER);
        let panel = get_page(&app, &cookie, "/ui/registration?notice=registered").await;
        assert!(panel.contains("You are registered."));
        assert!(panel.contains("Concurrency Test"));
        let enrollment_id = extract_input(&panel, "enrollment_id");

        // Drop via the form → redirect → panel is empty again.
        let response = actix_test::call_service(
            &app,
            actix_test::TestRequest::post()
                .uri("/ui/registration/drop")
                .cookie(cookie.clone())
                .set_form(serde_json::json!({
                    "csrf_token": csrf,
                    "enrollment_id": enrollment_id,
                }))
                .to_request(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let panel = get_page(&app, &cookie, "/ui/registration").await;
        assert!(panel.contains("not registered for any sections"));

        // The catalog search narrows: a nonsense query lists nothing.
        let filtered = get_page(&app, &cookie, "/ui/catalog?q=NO-SUCH-COURSE").await;
        assert!(filtered.contains("No open sections match."));
    }

    /// The enhanced write path: a register/drop POST with the `X-Fragment`
    /// header answers with the single re-rendered row in its COMMITTED
    /// state — the server outcome, never an optimistic "Enrolled". A denial
    /// answers 409 with the specific reason named in the row.
    #[sqlx::test(migrations = "./migrations")]
    async fn register_fragment_reflects_committed_server_outcome_never_optimistic(pool: PgPool) {
        let fixture = seed_registration_fixture(&pool, 1).await;
        let username = credential_student(&pool, &fixture).await;
        let app = ui_app!(&pool, fixture.registrar.institution_id);
        let cookie = login(&app, &username, PASSWORD).await;

        let catalog = get_page(&app, &cookie, "/ui/catalog").await;
        let csrf = extract_input(&catalog, "csrf_token");
        assert!(
            !catalog.contains("is-enrolled"),
            "nothing is enrolled before the server says so"
        );

        let post_fragment = |uri: &'static str, form: serde_json::Value| {
            let cookie = cookie.clone();
            let app = &app;
            async move {
                let response = actix_test::call_service(
                    app,
                    actix_test::TestRequest::post()
                        .uri(uri)
                        .cookie(cookie)
                        .insert_header(("X-Fragment", "row"))
                        .set_form(form)
                        .to_request(),
                )
                .await;
                let status = response.status();
                let body =
                    String::from_utf8(actix_test::read_body(response).await.to_vec()).unwrap();
                (status, body)
            }
        };

        // Success: the swapped-in row is the enrolled state, from the server.
        let (status, row) = post_fragment(
            "/ui/registration/add",
            serde_json::json!({
                "csrf_token": &csrf,
                "section_id": fixture.section_id,
                "idempotency_key": Uuid::new_v4(),
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(row.trim_start().starts_with("<tr"), "a row, not a page");
        assert!(row.contains("is-enrolled") && row.contains("Enrolled"));
        assert!(row.contains("/ui/registration/drop"), "row now offers Drop");

        // Denial: an honest 409 whose row names the specific reason and does
        // NOT paint an enrolled state it did not earn.
        let (status, denied) = post_fragment(
            "/ui/registration/add",
            serde_json::json!({
                "csrf_token": &csrf,
                "section_id": fixture.section_id,
                "idempotency_key": Uuid::new_v4(),
            }),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(
            denied.contains("already enrolled"),
            "the rejection names its reason: {denied}"
        );

        // Drop through the fragment path: the row returns to available.
        let enrollment_id = extract_input(&row, "enrollment_id");
        let (status, row) = post_fragment(
            "/ui/registration/drop",
            serde_json::json!({
                "csrf_token": &csrf,
                "enrollment_id": enrollment_id,
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(!row.contains("is-enrolled"));
        assert!(row.contains("/ui/registration/add"), "row offers Register");
    }

    /// Every rejection case in the student capability list renders inline
    /// feedback on the page (status 409), never a bare error blob.
    #[sqlx::test(migrations = "./migrations")]
    async fn every_rejection_case_renders_inline_feedback(pool: PgPool) {
        let fixture = seed_registration_fixture(&pool, 5).await;
        let username = credential_student(&pool, &fixture).await;
        let app = ui_app!(&pool, fixture.registrar.institution_id);
        let service =
            crate::enrollment::EnrollmentService::new(pool.clone(), crate::audit::AuditWriter);

        // Scenario sections.
        add_meeting(&pool, fixture.section_id, 1, "09:00:00", "10:00:00").await;
        let overlapping = add_course_section(&pool, &fixture, "OVERLAP", 5).await;
        add_meeting(&pool, overlapping, 1, "09:30:00", "10:30:00").await;
        let full = add_course_section(&pool, &fixture, "FULL", 0).await;
        let gated = add_course_section(&pool, &fixture, "ADVANCED", 5).await;
        let basic = add_course_section(&pool, &fixture, "BASIC", 5).await;
        let (gated_course, basic_course): (Uuid, Uuid) = (
            sqlx::query_scalar("SELECT course_id FROM section WHERE id = $1")
                .bind(gated)
                .fetch_one(&pool)
                .await
                .unwrap(),
            sqlx::query_scalar("SELECT course_id FROM section WHERE id = $1")
                .bind(basic)
                .fetch_one(&pool)
                .await
                .unwrap(),
        );
        sqlx::query(
            "INSERT INTO course_prerequisite (course_id, prerequisite_course_id, \
             minimum_grade_points) VALUES ($1, $2, 2.0)",
        )
        .bind(gated_course)
        .bind(basic_course)
        .execute(&pool)
        .await
        .unwrap();

        let cookie = login(&app, &username, PASSWORD).await;
        let csrf = extract_input(&get_page(&app, &cookie, "/ui/catalog").await, "csrf_token");
        let expect_inline = |status: StatusCode, body: String, message: &str| {
            assert_eq!(status, StatusCode::CONFLICT, "{message}: {body}");
            assert!(body.contains(message), "inline feedback missing: {message}");
            assert!(
                body.contains("<h1>My registration</h1>"),
                "full page rendered"
            );
        };

        // Closed window.
        sqlx::query("UPDATE academic_term SET add_drop_closes_at = now() - interval '1 minute'")
            .execute(&pool)
            .await
            .unwrap();
        let (status, body) = post_add(&app, &cookie, &csrf, fixture.section_id).await;
        expect_inline(status, body, "registration window is closed");
        sqlx::query("UPDATE academic_term SET add_drop_closes_at = now() + interval '7 days'")
            .execute(&pool)
            .await
            .unwrap();

        // Hold.
        service
            .place_hold(
                &fixture.registrar,
                fixture.student_a,
                fixture.term_id,
                "financial",
                "unpaid balance",
            )
            .await
            .unwrap();
        let (status, body) = post_add(&app, &cookie, &csrf, fixture.section_id).await;
        expect_inline(status, body, "student has a registration hold");
        service
            .release_hold(
                &fixture.registrar,
                fixture.student_a,
                fixture.term_id,
                "financial",
            )
            .await
            .unwrap();

        // Successful registration to set up duplicate + conflict.
        let (status, _) = post_add(&app, &cookie, &csrf, fixture.section_id).await;
        assert_eq!(status, StatusCode::SEE_OTHER);

        // Duplicate.
        let (status, body) = post_add(&app, &cookie, &csrf, fixture.section_id).await;
        expect_inline(status, body, "student is already enrolled in this section");

        // Schedule conflict.
        let (status, body) = post_add(&app, &cookie, &csrf, overlapping).await;
        expect_inline(status, body, "schedule conflict detected");

        // Full section.
        let (status, body) = post_add(&app, &cookie, &csrf, full).await;
        expect_inline(status, body, "section is full");

        // Unmet prerequisite.
        let (status, body) = post_add(&app, &cookie, &csrf, gated).await;
        expect_inline(status, body, "prerequisite requirements are not satisfied");
    }
}
