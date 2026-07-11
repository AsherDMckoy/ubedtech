#[sqlx::test(migrations = "./migrations")]
async fn only_one_student_gets_the_last_seat(pool: sqlx::PgPool) {
    // Arrange one section with capacity=1 and two eligible students.
    let fixture = seed_registration_fixture(&pool, 1).await;

    let service = crate::enrollment::EnrollmentService::new(
        pool.clone(),
        crate::audit::AuditWriter,
    );

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

    let enrolled_count: i32 = sqlx::query_scalar(
        "SELECT enrolled_count FROM section_capacity WHERE section_id = $1",
    )
    .bind(fixture.section_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(enrolled_count, 1);
}

async fn seed_registration_fixture(
    pool: &sqlx::PgPool,
    capacity: i32,
) -> Fixture {
    use std::collections::HashSet;
    use chrono::{Duration, Utc};
    use crate::shared::actor::{Actor, Role};
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

    sqlx::query(
        "INSERT INTO institution (id, code, name) VALUES ($1, $2, 'Test University')",
    )
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
            registration_opens_at, registration_closes_at, drop_add_closes_at
        )
        VALUES ($1, $2, $3, 'Test Term', $4, $5, $6, $7, $8)
        "#,
    )
    .bind(term_id)
    .bind(institution_id)
    .bind(format!("TERM-{}", &term_id.to_string()[..8]))
    .bind(now.date_naive())
    .bind((now + Duration::days(120)).date_naive())
    .bind(now - Duration::hours(1))
    .bind(now + Duration::hours(2))
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
