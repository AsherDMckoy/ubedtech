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
            Err(crate::shared::error::AppError::Conflict(message))
                if message.contains("section is full")
        ),
        "a full section is a Conflict: {full:?}"
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
            Err(crate::shared::error::AppError::Integrity(message))
                if message.contains("capacity")
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
            Err(crate::shared::error::AppError::Conflict(message))
                if message.contains("registration window")
        ),
        "late add must be refused: {late_add:?}"
    );

    let late_drop = service
        .drop_for(&fixture.registrar, fixture.student_a, receipt.enrollment_id)
        .await;
    assert!(
        matches!(&late_drop, Err(crate::shared::error::AppError::Conflict(_))),
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
    }
}

struct Fixture {
    registrar: crate::shared::actor::Actor,
    student_a: uuid::Uuid,
    student_b: uuid::Uuid,
    section_id: uuid::Uuid,
}
