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
        section_id,
        term_id,
    }
}

struct Fixture {
    registrar: crate::shared::actor::Actor,
    student_a: uuid::Uuid,
    student_b: uuid::Uuid,
    section_id: uuid::Uuid,
    term_id: uuid::Uuid,
}
