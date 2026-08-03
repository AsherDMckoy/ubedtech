//! Records integration tests: grade entry scoping, the publication and
//! correction workflows, revision history, and student visibility.

use chrono::{Duration, Utc};
use sqlx::PgPool;
use std::collections::HashSet;
use uuid::Uuid;

use crate::records::GradeService;
use crate::records::grades::{CorrectGradeCommand, SaveGradeCommand};
use crate::shared::actor::{Actor, Role};
use crate::shared::error::AppError;

struct GradeFixture {
    instructor: Actor,
    other_instructor: Actor,
    officer: Actor,
    student: Actor,
    section_id: Uuid,
    other_section_id: Uuid,
    term_id: Uuid,
    enrollment_id: Uuid,
}

fn actor(institution_id: Uuid, user_id: Uuid, role: Role, student_id: Option<Uuid>) -> Actor {
    Actor {
        user_id,
        institution_id,
        student_id,
        roles: HashSet::from([role]),
    }
}

async fn seed_user(pool: &PgPool, institution_id: Uuid) -> Uuid {
    let user_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO user_account (id, institution_id, username, email) VALUES ($1, $2, $3, $4)",
    )
    .bind(user_id)
    .bind(institution_id)
    .bind(format!("u-{}", &user_id.to_string()[..12]))
    .bind(format!("{}@test.invalid", &user_id.to_string()[..12]))
    .execute(pool)
    .await
    .unwrap();
    user_id
}

/// One institution: a term, a course, two sections (instructor assigned to
/// the first only), one enrolled student.
async fn seed_grade_fixture(pool: &PgPool) -> GradeFixture {
    let institution_id = Uuid::new_v4();
    sqlx::query("INSERT INTO institution (id, code, name) VALUES ($1, $2, 'Records U')")
        .bind(institution_id)
        .bind(format!("R-{}", &institution_id.to_string()[..8]))
        .execute(pool)
        .await
        .unwrap();

    let instructor_id = seed_user(pool, institution_id).await;
    let other_instructor_id = seed_user(pool, institution_id).await;
    let officer_id = seed_user(pool, institution_id).await;
    let student_user_id = seed_user(pool, institution_id).await;

    let student_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO student_profile (id, institution_id, user_id, student_number, program_code) \
         VALUES ($1, $2, $3, $4, 'CS')",
    )
    .bind(student_id)
    .bind(institution_id)
    .bind(student_user_id)
    .bind(format!("N-{}", &student_id.to_string()[..8]))
    .execute(pool)
    .await
    .unwrap();

    let term_id = Uuid::new_v4();
    let now = Utc::now();
    sqlx::query(
        "INSERT INTO academic_term (id, institution_id, code, name, starts_on, ends_on, \
         registration_opens_at, add_drop_closes_at, grade_entry_closes_at) \
         VALUES ($1, $2, $3, 'Term', $4, $5, $6, $7, $8)",
    )
    .bind(term_id)
    .bind(institution_id)
    .bind(format!("T-{}", &term_id.to_string()[..8]))
    .bind(now.date_naive())
    .bind((now + Duration::days(100)).date_naive())
    .bind(now - Duration::days(30))
    .bind(now + Duration::days(7))
    .bind(now + Duration::days(60))
    .execute(pool)
    .await
    .unwrap();

    let course_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO course (id, institution_id, code, title, credit_hours) \
         VALUES ($1, $2, $3, 'Records Course', 3.0)",
    )
    .bind(course_id)
    .bind(institution_id)
    .bind(format!("C-{}", &course_id.to_string()[..8]))
    .execute(pool)
    .await
    .unwrap();

    let section_id = Uuid::new_v4();
    let other_section_id = Uuid::new_v4();
    for (id, code) in [(section_id, "01"), (other_section_id, "02")] {
        sqlx::query(
            "INSERT INTO section (id, institution_id, term_id, course_id, section_code, status) \
             VALUES ($1, $2, $3, $4, $5, 'open')",
        )
        .bind(id)
        .bind(institution_id)
        .bind(term_id)
        .bind(course_id)
        .bind(code)
        .execute(pool)
        .await
        .unwrap();
    }

    sqlx::query(
        "INSERT INTO instructor_assignment (section_id, instructor_user_id) VALUES ($1, $2)",
    )
    .bind(section_id)
    .bind(instructor_id)
    .execute(pool)
    .await
    .unwrap();

    let enrollment_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO enrollment (id, institution_id, student_id, section_id, status, \
         registered_at, source, idempotency_key, created_by_user_id) \
         VALUES ($1, $2, $3, $4, 'enrolled', now(), 'registrar', $5, $6)",
    )
    .bind(enrollment_id)
    .bind(institution_id)
    .bind(student_id)
    .bind(section_id)
    .bind(Uuid::new_v4())
    .bind(officer_id)
    .execute(pool)
    .await
    .unwrap();

    GradeFixture {
        instructor: actor(institution_id, instructor_id, Role::Instructor, None),
        other_instructor: actor(institution_id, other_instructor_id, Role::Instructor, None),
        officer: actor(institution_id, officer_id, Role::RecordsOfficer, None),
        student: actor(
            institution_id,
            student_user_id,
            Role::Student,
            Some(student_id),
        ),
        section_id,
        other_section_id,
        term_id,
        enrollment_id,
    }
}

fn save_command(enrollment_id: Uuid, grade: &str, version: i64) -> SaveGradeCommand {
    SaveGradeCommand {
        enrollment_id,
        grade_code: grade.into(),
        grade_points: Some(3.0),
        numeric_value: None,
        expected_version: version,
    }
}

/// The phase's required proof: a correction preserves the prior value AND
/// its author in history, attributes the new value to the corrector, and
/// leaves the record `amended` — while drafts stay uncorrectable and
/// published grades stay un-redraftable.
#[sqlx::test(migrations = "./migrations")]
async fn corrections_preserve_prior_value_and_author_in_history(pool: PgPool) {
    let fx = seed_grade_fixture(&pool).await;
    let service = GradeService::new(pool.clone(), crate::audit::AuditWriter);

    // Instructor enters the draft; a draft cannot be "corrected".
    let version = service
        .save_draft(&fx.instructor, save_command(fx.enrollment_id, "B+", 0))
        .await
        .unwrap();
    let correct = |grade: &str, reason: &str, version: i64| {
        service.correct_grade(
            &fx.officer,
            CorrectGradeCommand {
                enrollment_id: fx.enrollment_id,
                grade_code: grade.into(),
                grade_points: Some(3.7),
                numeric_value: None,
                reason: reason.into(),
                expected_version: version,
            },
        )
    };
    assert!(matches!(
        correct("A-", "typo", version).await,
        Err(AppError::Conflict(_))
    ));

    service
        .publish_section(&fx.officer, fx.section_id)
        .await
        .unwrap();
    let published_version = version + 1;

    // Published grades cannot be quietly rewritten through draft entry —
    // by the instructor or by the officer.
    for who in [&fx.instructor, &fx.officer] {
        assert!(matches!(
            service
                .save_draft(who, save_command(fx.enrollment_id, "A+", published_version))
                .await,
            Err(AppError::Conflict(_))
        ));
    }

    // Guardrails on the correction itself.
    assert!(matches!(
        service
            .correct_grade(
                &fx.instructor,
                CorrectGradeCommand {
                    enrollment_id: fx.enrollment_id,
                    grade_code: "A-".into(),
                    grade_points: Some(3.7),
                    numeric_value: None,
                    reason: "instructor cannot".into(),
                    expected_version: published_version,
                },
            )
            .await,
        Err(AppError::Forbidden)
    ));
    assert!(matches!(
        correct("A-", "   ", published_version).await,
        Err(AppError::Validation(_))
    ));
    assert!(matches!(
        correct("A-", "stale", published_version + 5).await,
        Err(AppError::Conflict(_))
    ));

    // The real correction.
    correct("A-", "transcription error on the roster", published_version)
        .await
        .unwrap();

    let (state, grade_code, entered_by): (String, String, Uuid) = sqlx::query_as(
        "SELECT state, grade_code, entered_by_user_id FROM grade_record WHERE enrollment_id = $1",
    )
    .bind(fx.enrollment_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((state.as_str(), grade_code.as_str()), ("amended", "A-"));
    assert_eq!(entered_by, fx.officer.user_id);

    // History holds every prior version; the published B+ still carries the
    // instructor as its author.
    let history: Vec<(String, String, Uuid, i64)> = sqlx::query_as(
        "SELECT r.state, r.grade_code, r.entered_by_user_id, r.version \
         FROM grade_revision r JOIN grade_record g ON g.id = r.grade_record_id \
         WHERE g.enrollment_id = $1 ORDER BY r.version",
    )
    .bind(fx.enrollment_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(history.len(), 2, "draft->published and published->amended");
    let published = &history[1];
    assert_eq!(
        (published.0.as_str(), published.1.as_str(), published.2),
        ("published", "B+", fx.instructor.user_id),
    );

    // The correction is audited with its reason; grade rows cannot be
    // deleted at all.
    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_event WHERE action = 'grade.corrected' \
         AND actor_user_id = $1 AND detail->>'reason' IS NOT NULL",
    )
    .bind(fx.officer.user_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(audits, 1);
    assert!(
        sqlx::query("DELETE FROM grade_record WHERE enrollment_id = $1")
            .bind(fx.enrollment_id)
            .execute(&pool)
            .await
            .is_err(),
        "grade records are undeletable by trigger"
    );
}

/// Crafted-request scoping: an instructor who is not assigned to the
/// section cannot grade it no matter what enrollment id they post; students
/// cannot grade at all; another institution's officer sees 404. And the
/// student-visibility rule lives in the query: a draft grade is invisible
/// to the student, the published value appears, the amended value replaces
/// it.
#[sqlx::test(migrations = "./migrations")]
async fn unassigned_instructors_cannot_grade_and_students_never_see_drafts(pool: PgPool) {
    let fx = seed_grade_fixture(&pool).await;
    let service = GradeService::new(pool.clone(), crate::audit::AuditWriter);

    // Same institution, real instructor role, valid enrollment id — still
    // forbidden without an assignment to that section.
    assert!(matches!(
        service
            .save_draft(&fx.other_instructor, save_command(fx.enrollment_id, "A", 0))
            .await,
        Err(AppError::Forbidden)
    ));
    // A student posting the grading command directly is forbidden by role.
    assert!(matches!(
        service
            .save_draft(&fx.student, save_command(fx.enrollment_id, "A", 0))
            .await,
        Err(AppError::Forbidden)
    ));
    // A records officer of ANOTHER institution gets 404 for the foreign id.
    let foreign_institution = Uuid::new_v4();
    sqlx::query("INSERT INTO institution (id, code, name) VALUES ($1, $2, 'Other U')")
        .bind(foreign_institution)
        .bind(format!("O-{}", &foreign_institution.to_string()[..8]))
        .execute(&pool)
        .await
        .unwrap();
    let foreign_officer = actor(
        foreign_institution,
        seed_user(&pool, foreign_institution).await,
        Role::RecordsOfficer,
        None,
    );
    assert!(matches!(
        service
            .save_draft(&foreign_officer, save_command(fx.enrollment_id, "A", 0))
            .await,
        Err(AppError::NotFound)
    ));
    let grades: i64 = sqlx::query_scalar("SELECT count(*) FROM grade_record")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(grades, 0, "no denial wrote a grade");

    // Draft entered: the student's view is empty — the filter is in the
    // query itself, so there is no way to call it that returns a draft.
    service
        .save_draft(&fx.instructor, save_command(fx.enrollment_id, "B+", 0))
        .await
        .unwrap();
    let while_draft = service
        .student_grades(&fx.student, fx.term_id)
        .await
        .unwrap();
    assert!(while_draft.is_empty(), "draft grades are invisible");

    // Published: visible with the published value.
    service
        .publish_section(&fx.officer, fx.section_id)
        .await
        .unwrap();
    let published = service
        .student_grades(&fx.student, fx.term_id)
        .await
        .unwrap();
    assert_eq!(published.len(), 1);
    assert_eq!(published[0].grade_code, "B+");
    assert!(published[0].published_at.is_some());

    // Amended: the corrected value is what the student sees.
    service
        .correct_grade(
            &fx.officer,
            CorrectGradeCommand {
                enrollment_id: fx.enrollment_id,
                grade_code: "A-".into(),
                grade_points: Some(3.7),
                numeric_value: None,
                reason: "roster transcription error".into(),
                expected_version: 2,
            },
        )
        .await
        .unwrap();
    let amended = service
        .student_grades(&fx.student, fx.term_id)
        .await
        .unwrap();
    assert_eq!(amended.len(), 1);
    assert_eq!(amended[0].grade_code, "A-");
}

/// Rosters follow assignments: an instructor sees exactly their assigned
/// sections and nothing else — an unassigned section in the same institution
/// answers 404, indistinguishable from one that doesn't exist. The records
/// officer sees any section in their institution and no further.
#[sqlx::test(migrations = "./migrations")]
async fn rosters_are_visible_only_for_assigned_sections(pool: PgPool) {
    let fx = seed_grade_fixture(&pool).await;
    let service = GradeService::new(pool.clone(), crate::audit::AuditWriter);
    let other_section_id: Uuid =
        sqlx::query_scalar("SELECT id FROM section WHERE section_code = '02'")
            .fetch_one(&pool)
            .await
            .unwrap();

    // The assigned instructor: exactly one section listed, roster readable,
    // ungraded student shows as pending (no state).
    let sections = service.instructor_sections(&fx.instructor).await.unwrap();
    assert_eq!(sections.len(), 1);
    assert_eq!(sections[0].section_id, fx.section_id);
    assert_eq!(sections[0].enrolled_count, 1);

    let roster = service.roster(&fx.instructor, fx.section_id).await.unwrap();
    assert_eq!(roster.rows.len(), 1);
    assert!(roster.rows[0].state.is_none(), "no grade entered yet");
    assert_eq!(roster.section.section_code, "01");

    // Crafted requests: real section ids the instructor is NOT assigned to
    // answer 404 — same-institution sibling section and all of it for the
    // unassigned instructor.
    assert!(matches!(
        service.roster(&fx.instructor, other_section_id).await,
        Err(AppError::NotFound)
    ));
    assert!(matches!(
        service.roster(&fx.other_instructor, fx.section_id).await,
        Err(AppError::NotFound)
    ));
    assert!(
        service
            .instructor_sections(&fx.other_instructor)
            .await
            .unwrap()
            .is_empty()
    );
    // Students hold no roster power at all.
    assert!(matches!(
        service.roster(&fx.student, fx.section_id).await,
        Err(AppError::Forbidden)
    ));

    // The records officer reads any section in the institution; a foreign
    // officer reads none of them.
    assert_eq!(
        service
            .roster(&fx.officer, fx.section_id)
            .await
            .unwrap()
            .rows
            .len(),
        1
    );
    let foreign_institution = Uuid::new_v4();
    sqlx::query("INSERT INTO institution (id, code, name) VALUES ($1, $2, 'Other U')")
        .bind(foreign_institution)
        .bind(format!("O-{}", &foreign_institution.to_string()[..8]))
        .execute(&pool)
        .await
        .unwrap();
    let foreign_officer = actor(
        foreign_institution,
        seed_user(&pool, foreign_institution).await,
        Role::RecordsOfficer,
        None,
    );
    assert!(matches!(
        service.roster(&foreign_officer, fx.section_id).await,
        Err(AppError::NotFound)
    ));
}

/// Instructor assignment is registrar/institution-admin work, requires the
/// target to actually hold the instructor role, and is idempotent.
#[sqlx::test(migrations = "./migrations")]
async fn instructor_assignment_is_validated_scoped_and_idempotent(pool: PgPool) {
    let fx = seed_grade_fixture(&pool).await;
    let academics =
        crate::academics::AcademicsService::new(pool.clone(), crate::audit::AuditWriter);
    let registrar = actor(
        fx.officer.institution_id,
        seed_user(&pool, fx.officer.institution_id).await,
        Role::Registrar,
        None,
    );
    let other_section_id: Uuid =
        sqlx::query_scalar("SELECT id FROM section WHERE section_code = '02'")
            .fetch_one(&pool)
            .await
            .unwrap();

    // Neither instructors nor officers assign instructors.
    for who in [&fx.instructor, &fx.officer, &fx.student] {
        assert!(matches!(
            academics
                .assign_instructor(who, other_section_id, fx.other_instructor.user_id)
                .await,
            Err(AppError::Forbidden)
        ));
    }

    // The target must hold the instructor role (fx.officer does not).
    assert!(matches!(
        academics
            .assign_instructor(&registrar, other_section_id, fx.officer.user_id)
            .await,
        Err(AppError::Validation(_))
    ));

    // Role rows live in user_role: give other_instructor the real role, then
    // assignment works and unlocks the roster.
    sqlx::query(
        "INSERT INTO user_role (institution_id, user_id, role_id) \
         SELECT $1, $2, id FROM role WHERE code = 'instructor'",
    )
    .bind(registrar.institution_id)
    .bind(fx.other_instructor.user_id)
    .execute(&pool)
    .await
    .unwrap();
    academics
        .assign_instructor(&registrar, other_section_id, fx.other_instructor.user_id)
        .await
        .unwrap();
    assert_eq!(
        service_sections(&pool, &fx.other_instructor).await,
        vec![other_section_id]
    );

    // Idempotent: re-assigning writes no second audit row.
    academics
        .assign_instructor(&registrar, other_section_id, fx.other_instructor.user_id)
        .await
        .unwrap();
    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_event WHERE action = 'academics.instructor_assigned'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(audits, 1);
}

async fn service_sections(pool: &PgPool, instructor: &Actor) -> Vec<Uuid> {
    GradeService::new(pool.clone(), crate::audit::AuditWriter)
        .instructor_sections(instructor)
        .await
        .unwrap()
        .into_iter()
        .map(|section| section.section_id)
        .collect()
}

/// Transcript snapshots: records-officer command, monotonic versions,
/// published-only content, and database-enforced immutability.
#[sqlx::test(migrations = "./migrations")]
async fn transcript_snapshots_are_immutable_versioned_and_published_only(pool: PgPool) {
    let fx = seed_grade_fixture(&pool).await;
    let grades = GradeService::new(pool.clone(), crate::audit::AuditWriter);
    let snapshots = crate::records::TranscriptSnapshotService;

    // One published grade and one draft-only enrollment in a sibling section.
    grades
        .save_draft(&fx.instructor, save_command(fx.enrollment_id, "B+", 0))
        .await
        .unwrap();
    grades
        .publish_section(&fx.officer, fx.section_id)
        .await
        .unwrap();
    let other_section_id: Uuid =
        sqlx::query_scalar("SELECT id FROM section WHERE section_code = '02'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let draft_enrollment = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO enrollment (id, institution_id, student_id, section_id, status, \
         registered_at, source, idempotency_key, created_by_user_id) \
         VALUES ($1, $2, $3, $4, 'enrolled', now(), 'registrar', $5, $6)",
    )
    .bind(draft_enrollment)
    .bind(fx.officer.institution_id)
    .bind(fx.student.student_id.unwrap())
    .bind(other_section_id)
    .bind(Uuid::new_v4())
    .bind(fx.officer.user_id)
    .execute(&pool)
    .await
    .unwrap();
    grades
        .save_draft(&fx.officer, save_command(draft_enrollment, "C", 0))
        .await
        .unwrap();

    // Only the records officer generates snapshots; targets resolve inside
    // the institution.
    for who in [&fx.instructor, &fx.student] {
        assert!(matches!(
            snapshots
                .generate(
                    &pool,
                    &crate::audit::AuditWriter,
                    who,
                    fx.student.student_id.unwrap()
                )
                .await,
            Err(AppError::Forbidden)
        ));
    }
    assert!(matches!(
        snapshots
            .generate(
                &pool,
                &crate::audit::AuditWriter,
                &fx.officer,
                Uuid::new_v4()
            )
            .await,
        Err(AppError::NotFound)
    ));

    let snapshot_id = snapshots
        .generate(
            &pool,
            &crate::audit::AuditWriter,
            &fx.officer,
            fx.student.student_id.unwrap(),
        )
        .await
        .unwrap();

    // The snapshot holds exactly the published record — the draft C is not
    // in the artifact.
    let json: serde_json::Value =
        sqlx::query_scalar("SELECT snapshot_json FROM transcript_snapshot WHERE id = $1")
            .bind(snapshot_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let courses = json["courses"].as_array().unwrap();
    assert_eq!(courses.len(), 1);
    assert_eq!(courses[0]["grade_code"], "B+");

    // Immutable artifact: neither UPDATE nor DELETE survives the trigger.
    assert!(
        sqlx::query("UPDATE transcript_snapshot SET snapshot_json = '{}' WHERE id = $1")
            .bind(snapshot_id)
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM transcript_snapshot WHERE id = $1")
            .bind(snapshot_id)
            .execute(&pool)
            .await
            .is_err()
    );

    // A second snapshot is a new monotonic version, and the student sees
    // both in their own list (newest first).
    snapshots
        .generate(
            &pool,
            &crate::audit::AuditWriter,
            &fx.officer,
            fx.student.student_id.unwrap(),
        )
        .await
        .unwrap();
    let list = snapshots.own_snapshots(&pool, &fx.student).await.unwrap();
    assert_eq!(
        list.iter()
            .map(|snapshot| snapshot.snapshot_version)
            .collect::<Vec<_>>(),
        vec![2, 1]
    );

    // Generation is audited.
    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_event WHERE action = 'records.snapshot_created'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(audits, 2);
}

/// Academic history spans terms and shows only published/amended grades —
/// the same query-level rule as the per-term view.
#[sqlx::test(migrations = "./migrations")]
async fn academic_history_spans_terms_and_hides_drafts(pool: PgPool) {
    let fx = seed_grade_fixture(&pool).await;
    let grades = GradeService::new(pool.clone(), crate::audit::AuditWriter);

    grades
        .save_draft(&fx.instructor, save_command(fx.enrollment_id, "B+", 0))
        .await
        .unwrap();

    // Draft only: history is empty.
    assert!(
        grades
            .academic_history(&fx.student)
            .await
            .unwrap()
            .is_empty()
    );

    grades
        .publish_section(&fx.officer, fx.section_id)
        .await
        .unwrap();
    let history = grades.academic_history(&fx.student).await.unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].grade_code, "B+");
    assert_eq!(history[0].state, "published");

    // A non-student actor has no history to read.
    assert!(matches!(
        grades.academic_history(&fx.officer).await,
        Err(AppError::Forbidden)
    ));
}

/// The dashboard's GPA history: only COMPLETED terms count, only
/// published/amended grades count, and the weighting is by credit hours.
/// A term in progress never has a GPA row.
#[sqlx::test(migrations = "./migrations")]
async fn term_gpa_history_counts_only_completed_terms_and_published_grades(pool: PgPool) {
    let fx = seed_grade_fixture(&pool).await;
    let service = GradeService::new(pool.clone(), crate::audit::AuditWriter);

    // A published grade in the CURRENT term: no GPA row — the term is not
    // complete, so nothing about it is final.
    service
        .save_draft(&fx.instructor, save_command(fx.enrollment_id, "B", 0))
        .await
        .unwrap();
    service
        .publish_section(&fx.officer, fx.section_id)
        .await
        .unwrap();
    assert!(
        service.own_term_gpas(&fx.student).await.unwrap().is_empty(),
        "an in-progress term must not appear"
    );

    // The term ends: exactly one row, 3.0 GPA over the course's 3 credits.
    sqlx::query(
        "UPDATE academic_term SET starts_on = current_date - 100, ends_on = current_date - 1",
    )
    .execute(&pool)
    .await
    .unwrap();
    let terms = service.own_term_gpas(&fx.student).await.unwrap();
    assert_eq!(terms.len(), 1);
    assert_eq!(terms[0].credits, 3.0);
    assert!((terms[0].gpa() - 3.0).abs() < 1e-9);

    // A draft in the same completed term contributes nothing.
    let draft_enrollment = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO enrollment (id, institution_id, student_id, section_id, status, \
         registered_at, source, idempotency_key, created_by_user_id) \
         SELECT $1, institution_id, student_id, $2, 'enrolled', now(), 'registrar', $3, \
                created_by_user_id \
         FROM enrollment WHERE id = $4",
    )
    .bind(draft_enrollment)
    .bind(fx.other_section_id)
    .bind(Uuid::new_v4())
    .bind(fx.enrollment_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO grade_record (id, institution_id, enrollment_id, grade_code, \
         grade_points, state, entered_by_user_id) \
         SELECT $1, institution_id, $2, 'A', 4.0, 'draft', entered_by_user_id \
         FROM grade_record WHERE enrollment_id = $3",
    )
    .bind(Uuid::new_v4())
    .bind(draft_enrollment)
    .bind(fx.enrollment_id)
    .execute(&pool)
    .await
    .unwrap();
    let terms = service.own_term_gpas(&fx.student).await.unwrap();
    assert_eq!(terms[0].credits, 3.0, "the draft must not add credits");

    // Not a student: nothing to read.
    assert!(matches!(
        service.own_term_gpas(&fx.officer).await,
        Err(AppError::Forbidden)
    ));
}

/// The grade-entry window binds instructors; the records officer is the
/// late-entry escape hatch; a term without a deadline imposes none.
#[sqlx::test(migrations = "./migrations")]
async fn grade_entry_window_binds_instructors_not_the_officer(pool: PgPool) {
    let fx = seed_grade_fixture(&pool).await;
    let service = GradeService::new(pool.clone(), crate::audit::AuditWriter);

    sqlx::query("UPDATE academic_term SET grade_entry_closes_at = now() - interval '1 minute'")
        .execute(&pool)
        .await
        .unwrap();

    let late = service
        .save_draft(&fx.instructor, save_command(fx.enrollment_id, "B", 0))
        .await;
    assert!(
        matches!(&late, Err(AppError::Conflict(message)) if message.contains("window")),
        "late instructor entry refused: {late:?}"
    );

    let version = service
        .save_draft(&fx.officer, save_command(fx.enrollment_id, "B", 0))
        .await
        .expect("officer may enter after the window");

    // No deadline configured: instructors may enter.
    sqlx::query("UPDATE academic_term SET grade_entry_closes_at = NULL")
        .execute(&pool)
        .await
        .unwrap();
    service
        .save_draft(
            &fx.instructor,
            save_command(fx.enrollment_id, "B-", version),
        )
        .await
        .expect("no deadline means no window");
}

/// The no-draft-leak guarantee at the SERVICE layer: every student-facing
/// grade read excludes drafts in the query itself, so there is no calling
/// convention — page, JSON, future caller — that can surface one. (The
/// HTTP-level proof lives in the UI flow test; this pins the queries.)
#[sqlx::test(migrations = "./migrations")]
async fn draft_grades_never_reach_student_queries_however_called(pool: PgPool) {
    let fx = seed_grade_fixture(&pool).await;
    let service = GradeService::new(pool.clone(), crate::audit::AuditWriter);

    service
        .save_draft(&fx.instructor, save_command(fx.enrollment_id, "A", 0))
        .await
        .unwrap();
    let state: String =
        sqlx::query_scalar("SELECT state FROM grade_record WHERE enrollment_id = $1")
            .bind(fx.enrollment_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(state, "draft", "the grade exists, as a draft");

    assert!(
        service
            .student_grades(&fx.student, fx.term_id)
            .await
            .unwrap()
            .is_empty(),
        "current-term view must not return the draft"
    );
    assert!(
        service
            .academic_history(&fx.student)
            .await
            .unwrap()
            .is_empty(),
        "academic history must not return the draft"
    );

    // Publication is the one gate that makes it visible.
    service
        .publish_section(&fx.officer, fx.section_id)
        .await
        .unwrap();
    assert_eq!(
        service
            .student_grades(&fx.student, fx.term_id)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        service.academic_history(&fx.student).await.unwrap().len(),
        1
    );
}

/// The grade-history view walks the full lifecycle newest-first with each
/// value attributed, and obeys roster scoping: an unassigned instructor
/// gets 404 for a real enrollment id, a student is forbidden outright.
#[sqlx::test(migrations = "./migrations")]
async fn grade_history_shows_attributed_lifecycle_under_roster_scoping(pool: PgPool) {
    let fx = seed_grade_fixture(&pool).await;
    let service = GradeService::new(pool.clone(), crate::audit::AuditWriter);

    let version = service
        .save_draft(&fx.instructor, save_command(fx.enrollment_id, "B", 0))
        .await
        .unwrap();
    service
        .save_draft(
            &fx.instructor,
            save_command(fx.enrollment_id, "B+", version),
        )
        .await
        .unwrap();
    service
        .publish_section(&fx.officer, fx.section_id)
        .await
        .unwrap();
    service
        .correct_grade(
            &fx.officer,
            CorrectGradeCommand {
                enrollment_id: fx.enrollment_id,
                grade_code: "A-".into(),
                grade_points: Some(3.7),
                numeric_value: None,
                reason: "transcription error".into(),
                expected_version: version + 2,
            },
        )
        .await
        .unwrap();

    let history = service
        .grade_history(&fx.instructor, fx.enrollment_id)
        .await
        .unwrap();
    let states: Vec<(&str, &str, i64)> = history
        .entries
        .iter()
        .map(|entry| {
            (
                entry.grade_code.as_str(),
                entry.state.as_str(),
                entry.version,
            )
        })
        .collect();
    assert_eq!(
        states,
        vec![
            ("A-", "amended", 4),
            ("B+", "published", 3),
            ("B+", "draft", 2),
            ("B", "draft", 1),
        ],
        "every prior value survives, newest first"
    );
    // Attribution: the draft belongs to the instructor, the correction to
    // the officer.
    assert_ne!(history.entries[0].entered_by, history.entries[3].entered_by);
    assert_eq!(history.head.student_number.len(), 10);

    // Scoping mirrors the roster.
    assert!(matches!(
        service
            .grade_history(&fx.other_instructor, fx.enrollment_id)
            .await,
        Err(AppError::NotFound)
    ));
    assert!(matches!(
        service.grade_history(&fx.student, fx.enrollment_id).await,
        Err(AppError::Forbidden)
    ));
}

// ---------------------------------------------------------------------------
// Grade pages over plain forms: instructor entry, officer publish, student
// views — no JavaScript anywhere in the flow.
// ---------------------------------------------------------------------------

mod ui {
    use super::{GradeFixture, seed_grade_fixture};
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

    macro_rules! records_ui_app {
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
                    .app_data(web::Data::new($pool.clone()))
                    .app_data(web::Data::new(crate::academics::AcademicsService::new(
                        $pool.clone(),
                        crate::audit::AuditWriter,
                    )))
                    .app_data(web::Data::new(crate::records::GradeService::new(
                        $pool.clone(),
                        crate::audit::AuditWriter,
                    )))
                    .app_data(web::Data::new(crate::records::TranscriptSnapshotService))
                    .app_data(web::Data::new(crate::records::ScheduleQuery::new(
                        $pool.clone(),
                    )))
                    .app_data(web::Data::new(crate::enrollment::EnrollmentService::new(
                        $pool.clone(),
                        crate::audit::AuditWriter,
                    )))
                    .app_data(web::Data::new(crate::institution::InstitutionService::new(
                        $pool.clone(),
                        crate::audit::AuditWriter,
                    )))
                    .wrap(actix_web::middleware::from_fn(
                        crate::identity_access::csrf::csrf_middleware,
                    ))
                    .wrap(actix_web::middleware::from_fn(
                        crate::identity_access::middleware::session_middleware,
                    ))
                    .configure(crate::identity_access::http::routes)
                    .configure(crate::records::http::routes),
            )
            .await
        }};
    }

    /// Give a fixture user a known username, a credential, and their role.
    async fn credential(pool: &PgPool, user_id: Uuid, institution: Uuid, role: &str) -> String {
        let username = format!("{role}-{}", &user_id.to_string()[..8]);
        sqlx::query("UPDATE user_account SET username = $2 WHERE id = $1")
            .bind(user_id)
            .bind(&username)
            .execute(pool)
            .await
            .unwrap();
        let hash = PasswordService::new(8, 1, 1)
            .unwrap()
            .hash(PASSWORD)
            .unwrap();
        sqlx::query("INSERT INTO password_credential (user_id, password_hash) VALUES ($1, $2)")
            .bind(user_id)
            .bind(hash)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO user_role (institution_id, user_id, role_id) \
             SELECT $1, $2, id FROM role WHERE code = $3",
        )
        .bind(institution)
        .bind(user_id)
        .bind(role)
        .execute(pool)
        .await
        .unwrap();
        username
    }

    async fn login<S, B>(app: &S, username: &str) -> actix_web::cookie::Cookie<'static>
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
                .set_form(serde_json::json!({ "username": username, "password": PASSWORD }))
                .to_request(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        response
            .response()
            .cookies()
            .find(|cookie| cookie.name() == "ub_session")
            .expect("session cookie")
            .into_owned()
    }

    async fn get<S, B>(
        app: &S,
        cookie: &actix_web::cookie::Cookie<'static>,
        uri: &str,
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
            actix_test::TestRequest::get()
                .uri(uri)
                .cookie(cookie.clone())
                .to_request(),
        )
        .await;
        let status = response.status();
        let body = String::from_utf8(actix_test::read_body(response).await.to_vec()).unwrap();
        if status == StatusCode::OK {
            crate::shared::assets::assert_page_a11y(&body);
        }
        (status, body)
    }

    fn extract_input(body: &str, name: &str) -> String {
        let at = body
            .find(&format!("name=\"{name}\""))
            .unwrap_or_else(|| panic!("no input named {name} in page"));
        let rest = &body[at..];
        let value_at = rest.find("value=\"").expect("input has a value") + "value=\"".len();
        rest[value_at..].split('"').next().unwrap().to_owned()
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn instructor_enters_officer_publishes_student_sees_published_only(pool: PgPool) {
        let fx: GradeFixture = seed_grade_fixture(&pool).await;
        let institution = fx.officer.institution_id;
        let instructor_login =
            credential(&pool, fx.instructor.user_id, institution, "instructor").await;
        let officer_login =
            credential(&pool, fx.officer.user_id, institution, "records_officer").await;
        let student_login = credential(&pool, fx.student.user_id, institution, "student").await;
        let app = records_ui_app!(&pool, institution);
        let other_section: Uuid =
            sqlx::query_scalar("SELECT id FROM section WHERE section_code = '02'")
                .fetch_one(&pool)
                .await
                .unwrap();

        // Instructor: section list links to the roster; the student is
        // pending; the unassigned sibling section is a 404 even though the
        // id is real.
        let instructor = login(&app, &instructor_login).await;
        let (status, body) = get(&app, &instructor, "/ui/instructor").await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("Records Course"));
        let (status, roster) = get(
            &app,
            &instructor,
            &format!("/ui/instructor/sections/{}", fx.section_id),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(roster.contains("Not entered"));
        assert!(
            !roster.contains("/publish"),
            "instructors get no publish form"
        );
        // The section switcher lists ONLY this instructor's assignments —
        // the sibling section's real id never appears in the menu.
        assert!(roster.contains("Switch section"));
        assert!(
            !roster.contains(&other_section.to_string()),
            "switcher must not offer unassigned sections"
        );
        let (status, _) = get(
            &app,
            &instructor,
            &format!("/ui/instructor/sections/{other_section}"),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        // Enter a draft through the form.
        let csrf = extract_input(&roster, "csrf_token");
        let response = actix_test::call_service(
            &app,
            actix_test::TestRequest::post()
                .uri("/ui/instructor/grades")
                .cookie(instructor.clone())
                .set_form(serde_json::json!({
                    "csrf_token": csrf,
                    "section_id": fx.section_id,
                    "enrollment_id": fx.enrollment_id,
                    "grade_code": "B+",
                    "grade_points": "3.3",
                    "expected_version": 0,
                }))
                .to_request(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let (_, roster) = get(
            &app,
            &instructor,
            &format!("/ui/instructor/sections/{}?notice=saved", fx.section_id),
        )
        .await;
        assert!(roster.contains("draft") && roster.contains("B+"));
        assert!(roster.contains("Draft grade saved."));

        // Student: the draft is invisible on every page; instructor pages
        // are forbidden outright.
        let student = login(&app, &student_login).await;
        let (status, body) = get(&app, &student, "/ui/grades").await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("No published grades yet"));
        assert!(!body.contains("B+"), "draft grade must not leak: {body}");
        let (status, _) = get(&app, &student, "/ui/instructor").await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        // Officer: sees the roster with a publish button and publishes.
        let officer = login(&app, &officer_login).await;
        let (_, officer_roster) = get(
            &app,
            &officer,
            &format!("/ui/instructor/sections/{}", fx.section_id),
        )
        .await;
        assert!(officer_roster.contains("Publish 1 draft"));
        let officer_csrf = extract_input(&officer_roster, "csrf_token");
        let response = actix_test::call_service(
            &app,
            actix_test::TestRequest::post()
                .uri(&format!(
                    "/ui/instructor/sections/{}/publish",
                    fx.section_id
                ))
                .cookie(officer.clone())
                .set_form(serde_json::json!({ "csrf_token": officer_csrf }))
                .to_request(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);

        // Published rows lock their entry form on the roster.
        let (_, roster) = get(
            &app,
            &officer,
            &format!("/ui/instructor/sections/{}", fx.section_id),
        )
        .await;
        assert!(roster.contains("published"));
        assert!(
            roster.contains("grade-final") && !roster.contains("name=\"grade_code\""),
            "published rows are read-only"
        );

        // The row links its grade history; the page walks draft → published
        // with attribution (audited for a11y by get()).
        let history_uri = format!(
            "/ui/instructor/enrollments/{}/grade-history",
            fx.enrollment_id
        );
        assert!(roster.contains(&history_uri));
        let (status, grade_history) = get(&app, &instructor, &history_uri).await;
        assert_eq!(status, StatusCode::OK);
        assert!(grade_history.contains("B+"));
        assert!(grade_history.contains(">draft<") && grade_history.contains(">published<"));

        // Student now sees the published grade, and the history page shows
        // the record with no snapshots yet.
        let (status, body) = get(&app, &student, "/ui/grades").await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("B+"));
        // History is merged into the grades page: the published record
        // and the snapshots section render beneath this term's grades.
        assert!(body.contains("published"));
        assert!(body.contains("No transcript snapshots"));

        // Schedule: the enrolled section's meeting lands on its weekday.
        sqlx::query(
            "INSERT INTO section_meeting (id, section_id, day_of_week, starts_at, ends_at) \
             VALUES ($1, $2, 2, '09:00:00', '10:15:00')",
        )
        .bind(Uuid::new_v4())
        .bind(fx.section_id)
        .execute(&pool)
        .await
        .unwrap();
        let (status, schedule) = get(&app, &student, "/ui/schedule").await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            schedule.contains("Tuesday") && schedule.contains("Records Course"),
            "meeting shows on its weekday"
        );

        // Unofficial transcript: identity, the published grade, and the
        // unofficial marking — for the student's own record only.
        let (status, transcript) = get(&app, &student, "/ui/transcript").await;
        assert_eq!(status, StatusCode::OK);
        assert!(transcript.contains("Unofficial transcript"));
        assert!(transcript.contains("B+") && transcript.contains("Records Course"));
        assert!(
            transcript.contains("not an official university document"),
            "transcript marks itself unofficial"
        );

        // Proof of enrollment: identity plus the active enrollment.
        let (status, proof) = get(&app, &student, "/ui/proof-of-enrollment").await;
        assert_eq!(status, StatusCode::OK);
        assert!(proof.contains("Proof of enrollment"));
        assert!(proof.contains("Records Course"));
        assert!(proof.contains("not an official university document"));

        // Staff without a student profile have no student documents.
        let (status, _) = get(&app, &officer, "/ui/transcript").await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    /// No optimistic publish: the roster shows "published" only after the
    /// server has committed it. A crafted instructor publish is refused and
    /// flips nothing; the officer's publish flips the row only on the
    /// post-redirect re-render.
    #[sqlx::test(migrations = "./migrations")]
    async fn publish_never_flips_before_the_server_commits(pool: PgPool) {
        let fx: GradeFixture = seed_grade_fixture(&pool).await;
        let institution = fx.officer.institution_id;
        let instructor_login =
            credential(&pool, fx.instructor.user_id, institution, "instructor").await;
        let officer_login =
            credential(&pool, fx.officer.user_id, institution, "records_officer").await;
        let app = records_ui_app!(&pool, institution);

        // Instructor enters a draft through the select form (no explicit
        // points: the standard scale supplies them, assumption A29).
        let instructor = login(&app, &instructor_login).await;
        let (_, roster) = get(
            &app,
            &instructor,
            &format!("/ui/instructor/sections/{}", fx.section_id),
        )
        .await;
        let csrf = extract_input(&roster, "csrf_token");
        let publish_uri = format!("/ui/instructor/sections/{}/publish", fx.section_id);
        let response = actix_test::call_service(
            &app,
            actix_test::TestRequest::post()
                .uri("/ui/instructor/grades")
                .cookie(instructor.clone())
                .set_form(serde_json::json!({
                    "csrf_token": csrf,
                    "section_id": fx.section_id,
                    "enrollment_id": fx.enrollment_id,
                    "grade_code": "B+",
                    "expected_version": 0,
                }))
                .to_request(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let points: Option<f64> =
            sqlx::query_scalar("SELECT grade_points FROM grade_record WHERE enrollment_id = $1")
                .bind(fx.enrollment_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(points, Some(3.3), "standard scale supplied the points");

        // Draft state on screen; nothing published.
        let (_, roster) = get(
            &app,
            &instructor,
            &format!("/ui/instructor/sections/{}", fx.section_id),
        )
        .await;
        assert!(roster.contains(">draft<") && !roster.contains("grade-final"));

        // Crafted instructor publish: refused, and the roster still shows a
        // draft — no flip without a committed outcome.
        let response = actix_test::call_service(
            &app,
            actix_test::TestRequest::post()
                .uri(&publish_uri)
                .cookie(instructor.clone())
                .set_form(serde_json::json!({ "csrf_token": csrf }))
                .to_request(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let (_, roster) = get(
            &app,
            &instructor,
            &format!("/ui/instructor/sections/{}", fx.section_id),
        )
        .await;
        assert!(roster.contains(">draft<") && !roster.contains("grade-final"));

        // Officer publishes; only the post-commit re-render shows published.
        let officer = login(&app, &officer_login).await;
        let (_, officer_roster) = get(
            &app,
            &officer,
            &format!("/ui/instructor/sections/{}", fx.section_id),
        )
        .await;
        let officer_csrf = extract_input(&officer_roster, "csrf_token");
        let response = actix_test::call_service(
            &app,
            actix_test::TestRequest::post()
                .uri(&publish_uri)
                .cookie(officer.clone())
                .set_form(serde_json::json!({ "csrf_token": officer_csrf }))
                .to_request(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let (_, roster) = get(
            &app,
            &instructor,
            &format!("/ui/instructor/sections/{}", fx.section_id),
        )
        .await;
        assert!(roster.contains("grade-final") && roster.contains("published"));
    }

    /// Outside the entry window the roster is read-only with a plain
    /// explanation; a denial on save renders inline, associated with the
    /// row it belongs to.
    #[sqlx::test(migrations = "./migrations")]
    async fn closed_window_disables_entry_and_denials_render_inline(pool: PgPool) {
        let fx: GradeFixture = seed_grade_fixture(&pool).await;
        let institution = fx.officer.institution_id;
        let instructor_login =
            credential(&pool, fx.instructor.user_id, institution, "instructor").await;
        let app = records_ui_app!(&pool, institution);
        let instructor = login(&app, &instructor_login).await;
        let roster_uri = format!("/ui/instructor/sections/{}", fx.section_id);

        // Window open: the entry select renders.
        let (_, roster) = get(&app, &instructor, &roster_uri).await;
        assert!(roster.contains("Grade entry open"));
        assert!(roster.contains("name=\"grade_code\""));
        let csrf = extract_input(&roster, "csrf_token");

        // Stale version: 409 whose message sits inline in the row.
        let response = actix_test::call_service(
            &app,
            actix_test::TestRequest::post()
                .uri("/ui/instructor/grades")
                .cookie(instructor.clone())
                .set_form(serde_json::json!({
                    "csrf_token": csrf,
                    "section_id": fx.section_id,
                    "enrollment_id": fx.enrollment_id,
                    "grade_code": "B+",
                    "expected_version": 99,
                }))
                .to_request(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let body = String::from_utf8(actix_test::read_body(response).await.to_vec()).unwrap();
        assert!(
            body.contains(&format!("id=\"err-{}\"", fx.enrollment_id)),
            "denial is associated with its row: {body}"
        );

        // Window closed: inputs give way to read-only values + explanation.
        sqlx::query("UPDATE academic_term SET grade_entry_closes_at = now() - interval '1 hour'")
            .execute(&pool)
            .await
            .unwrap();
        let (_, roster) = get(&app, &instructor, &roster_uri).await;
        assert!(roster.contains("Grade entry closed"));
        assert!(!roster.contains("name=\"grade_code\""), "no entry controls");
    }
}
