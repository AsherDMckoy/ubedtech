//! Integration tests for the academic-structure commands and catalog reads:
//! roles, two-institution scoping, unique-code conflicts, and the
//! section+capacity transactional guarantee.

use chrono::{Duration, Utc};
use sqlx::PgPool;
use std::collections::HashSet;
use uuid::Uuid;

use crate::academics::AcademicsService;
use crate::academics::service::{
    AddMeetingCommand, AddPrerequisiteCommand, CreateCourseCommand, CreateSectionCommand,
    CreateTermCommand,
};
use crate::shared::actor::{Actor, Role};
use crate::shared::error::AppError;

async fn seed_actor(pool: &PgPool, role: Role) -> Actor {
    let institution_id = Uuid::new_v4();
    let user_id = Uuid::new_v4();
    sqlx::query("INSERT INTO institution (id, code, name) VALUES ($1, $2, 'Academics U')")
        .bind(institution_id)
        .bind(format!("A-{}", &institution_id.to_string()[..8]))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO user_account (id, institution_id, username, email) VALUES ($1, $2, $3, $4)",
    )
    .bind(user_id)
    .bind(institution_id)
    .bind(format!("u-{}", &user_id.to_string()[..8]))
    .bind(format!("{}@test.invalid", &user_id.to_string()[..8]))
    .execute(pool)
    .await
    .unwrap();

    Actor {
        user_id,
        institution_id,
        student_id: None,
        roles: HashSet::from([role]),
    }
}

fn term_command(code: &str) -> CreateTermCommand {
    let now = Utc::now();
    CreateTermCommand {
        code: code.into(),
        name: "Fall".into(),
        starts_on: now.date_naive(),
        ends_on: (now + Duration::days(100)).date_naive(),
        registration_opens_at: now - Duration::days(1),
        add_drop_closes_at: now + Duration::days(14),
        grade_entry_closes_at: None,
    }
}

fn course_command(code: &str) -> CreateCourseCommand {
    CreateCourseCommand {
        code: code.into(),
        title: "Intro".into(),
        credit_hours: 3.0,
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn academics_commands_enforce_the_role_matrix(pool: PgPool) {
    let service = AcademicsService::new(pool.clone(), crate::audit::AuditWriter);

    // Denied roles get 403 and write nothing.
    for role in [Role::Student, Role::Instructor, Role::RecordsOfficer] {
        let actor = seed_actor(&pool, role).await;
        assert!(
            matches!(
                service.create_term(&actor, term_command("T1")).await,
                Err(AppError::Forbidden)
            ),
            "role {role:?} must not create terms"
        );
    }
    let terms: i64 = sqlx::query_scalar("SELECT count(*) FROM academic_term")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(terms, 0);

    // Registrar and institution admin both may.
    for role in [Role::Registrar, Role::InstitutionAdmin] {
        let actor = seed_actor(&pool, role).await;
        service
            .create_term(&actor, term_command("T1"))
            .await
            .unwrap_or_else(|e| panic!("role {role:?} creates terms: {e:?}"));
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn codes_are_unique_per_institution_not_globally(pool: PgPool) {
    let service = AcademicsService::new(pool.clone(), crate::audit::AuditWriter);
    let registrar_a = seed_actor(&pool, Role::Registrar).await;
    let registrar_b = seed_actor(&pool, Role::Registrar).await;

    service
        .create_term(&registrar_a, term_command("FALL"))
        .await
        .unwrap();
    // Same code again in the same institution: a client-visible conflict.
    assert!(matches!(
        service
            .create_term(&registrar_a, term_command("FALL"))
            .await,
        Err(AppError::Conflict(_))
    ));
    // Same code in another institution: fine.
    service
        .create_term(&registrar_b, term_command("FALL"))
        .await
        .unwrap();

    service
        .create_course(&registrar_a, course_command("CS101"))
        .await
        .unwrap();
    assert!(matches!(
        service
            .create_course(&registrar_a, course_command("CS101"))
            .await,
        Err(AppError::Conflict(_))
    ));
    service
        .create_course(&registrar_b, course_command("CS101"))
        .await
        .unwrap();
}

#[sqlx::test(migrations = "./migrations")]
async fn section_creation_sets_capacity_in_the_same_transaction(pool: PgPool) {
    let service = AcademicsService::new(pool.clone(), crate::audit::AuditWriter);
    let registrar = seed_actor(&pool, Role::Registrar).await;
    let outsider = seed_actor(&pool, Role::Registrar).await;

    let term_id = service
        .create_term(&registrar, term_command("FALL"))
        .await
        .unwrap();
    let course_id = service
        .create_course(&registrar, course_command("CS101"))
        .await
        .unwrap();

    let section_id = service
        .create_section(
            &registrar,
            CreateSectionCommand {
                term_id,
                course_id,
                section_code: "01".into(),
                capacity: 30,
            },
        )
        .await
        .unwrap();

    let (capacity, enrolled): (i32, i32) = sqlx::query_as(
        "SELECT capacity, enrolled_count FROM section_capacity WHERE section_id = $1",
    )
    .bind(section_id)
    .fetch_one(&pool)
    .await
    .expect("capacity row exists with the section");
    assert_eq!((capacity, enrolled), (30, 0));

    // Parents from another institution answer 404 — even when the ids exist.
    assert!(matches!(
        service
            .create_section(
                &outsider,
                CreateSectionCommand {
                    term_id,
                    course_id,
                    section_code: "01".into(),
                    capacity: 10,
                },
            )
            .await,
        Err(AppError::NotFound)
    ));

    // Capacity adjustments are institution-scoped too.
    service
        .set_section_capacity(&registrar, section_id, 5)
        .await
        .unwrap();
    assert!(matches!(
        service
            .set_section_capacity(&outsider, section_id, 99)
            .await,
        Err(AppError::NotFound)
    ));

    // Audit trail: one event per successful command, none for the denials.
    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_event WHERE institution_id = $1 \
         AND action LIKE 'academics.%'",
    )
    .bind(registrar.institution_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(audits, 4); // term, course, section, capacity_set
    let outsider_audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_event WHERE institution_id = $1 \
         AND action LIKE 'academics.%'",
    )
    .bind(outsider.institution_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(outsider_audits, 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn capacity_cannot_shrink_below_enrollment(pool: PgPool) {
    let service = AcademicsService::new(pool.clone(), crate::audit::AuditWriter);
    let registrar = seed_actor(&pool, Role::Registrar).await;

    let term_id = service
        .create_term(&registrar, term_command("FALL"))
        .await
        .unwrap();
    let course_id = service
        .create_course(&registrar, course_command("CS101"))
        .await
        .unwrap();
    let section_id = service
        .create_section(
            &registrar,
            CreateSectionCommand {
                term_id,
                course_id,
                section_code: "01".into(),
                capacity: 2,
            },
        )
        .await
        .unwrap();

    sqlx::query("UPDATE section_capacity SET enrolled_count = 2 WHERE section_id = $1")
        .bind(section_id)
        .execute(&pool)
        .await
        .unwrap();

    assert!(matches!(
        service
            .set_section_capacity(&registrar, section_id, 1)
            .await,
        Err(AppError::Conflict(_))
    ));
    service
        .set_section_capacity(&registrar, section_id, 2)
        .await
        .expect("capacity equal to enrollment is allowed");
}

#[sqlx::test(migrations = "./migrations")]
async fn meetings_and_prerequisites_validate_their_inputs(pool: PgPool) {
    let service = AcademicsService::new(pool.clone(), crate::audit::AuditWriter);
    let registrar = seed_actor(&pool, Role::Registrar).await;

    let term_id = service
        .create_term(&registrar, term_command("FALL"))
        .await
        .unwrap();
    let course_id = service
        .create_course(&registrar, course_command("CS101"))
        .await
        .unwrap();
    let section_id = service
        .create_section(
            &registrar,
            CreateSectionCommand {
                term_id,
                course_id,
                section_code: "01".into(),
                capacity: 10,
            },
        )
        .await
        .unwrap();

    let meeting = |day: i16, start: &str, end: &str| AddMeetingCommand {
        day_of_week: day,
        starts_at: start.parse().unwrap(),
        ends_at: end.parse().unwrap(),
        room_id: None,
    };

    assert!(matches!(
        service
            .add_meeting(&registrar, section_id, meeting(8, "09:00:00", "10:00:00"))
            .await,
        Err(AppError::Validation(_))
    ));
    assert!(matches!(
        service
            .add_meeting(&registrar, section_id, meeting(1, "10:00:00", "09:00:00"))
            .await,
        Err(AppError::Validation(_))
    ));
    // An unknown room 404s rather than inserting a dangling reference.
    assert!(matches!(
        service
            .add_meeting(
                &registrar,
                section_id,
                AddMeetingCommand {
                    room_id: Some(Uuid::new_v4()),
                    ..meeting(1, "09:00:00", "10:00:00")
                },
            )
            .await,
        Err(AppError::NotFound)
    ));
    service
        .add_meeting(&registrar, section_id, meeting(1, "09:00:00", "10:00:00"))
        .await
        .expect("valid meeting");

    assert!(matches!(
        service
            .add_prerequisite(
                &registrar,
                course_id,
                AddPrerequisiteCommand {
                    prerequisite_course_id: course_id,
                    minimum_grade_points: 1.0,
                },
            )
            .await,
        Err(AppError::Validation(_))
    ));
    let other_course = service
        .create_course(&registrar, course_command("CS100"))
        .await
        .unwrap();
    service
        .add_prerequisite(
            &registrar,
            course_id,
            AddPrerequisiteCommand {
                prerequisite_course_id: other_course,
                minimum_grade_points: 1.0,
            },
        )
        .await
        .expect("valid prerequisite");
}

#[sqlx::test(migrations = "./migrations")]
async fn catalog_and_current_term_are_institution_scoped(pool: PgPool) {
    let service = AcademicsService::new(pool.clone(), crate::audit::AuditWriter);
    let registrar_a = seed_actor(&pool, Role::Registrar).await;
    let registrar_b = seed_actor(&pool, Role::Registrar).await;

    let term_a = service
        .create_term(&registrar_a, term_command("FALL"))
        .await
        .unwrap();
    let course_a = service
        .create_course(&registrar_a, course_command("CS101"))
        .await
        .unwrap();
    service
        .create_section(
            &registrar_a,
            CreateSectionCommand {
                term_id: term_a,
                course_id: course_a,
                section_code: "01".into(),
                capacity: 30,
            },
        )
        .await
        .unwrap();

    // Institution A sees its section; a search for a course it has not
    // offered matches nothing.
    let found = service
        .search_catalog(&registrar_a, term_a, Some("cs1"), 0)
        .await
        .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].course_code, "CS101");
    assert_eq!(found[0].capacity, 30);
    let none = service
        .search_catalog(&registrar_a, term_a, Some("BIOLOGY"), 0)
        .await
        .unwrap();
    assert!(none.is_empty());

    // Institution B, asking for A's term id, sees nothing at all.
    let cross = service
        .search_catalog(&registrar_b, term_a, None, 0)
        .await
        .unwrap();
    assert!(cross.is_empty());

    // current_term: A has one; B has none.
    assert_eq!(
        service
            .current_term(&registrar_a)
            .await
            .unwrap()
            .unwrap()
            .id,
        term_a
    );
    assert!(service.current_term(&registrar_b).await.unwrap().is_none());
}
