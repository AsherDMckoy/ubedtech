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
    assert_eq!(roster.len(), 1);
    assert!(roster[0].state.is_none(), "no grade entered yet");

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
