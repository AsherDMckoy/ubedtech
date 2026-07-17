//! Institution-administration tests: events/holidays are admin-only,
//! validated, institution-scoped, and audited in the same transaction.

use chrono::NaiveDate;
use sqlx::PgPool;
use std::collections::HashSet;
use uuid::Uuid;

use crate::institution::InstitutionService;
use crate::institution::service::EventCommand;
use crate::shared::{
    actor::{Actor, Role},
    error::AppError,
};

pub(super) struct AdminFixture {
    pub admin: Actor,
    pub student: Actor,
    pub foreign_admin: Actor,
}

fn actor(institution_id: Uuid, user_id: Uuid, role: Role) -> Actor {
    Actor {
        user_id,
        institution_id,
        student_id: None,
        roles: HashSet::from([role]),
    }
}

async fn seed_institution(pool: &PgPool) -> Uuid {
    let institution_id = Uuid::new_v4();
    sqlx::query("INSERT INTO institution (id, code, name) VALUES ($1, $2, 'Admin U')")
        .bind(institution_id)
        .bind(format!("A-{}", &institution_id.to_string()[..8]))
        .execute(pool)
        .await
        .unwrap();
    institution_id
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

pub(super) async fn seed_admin_fixture(pool: &PgPool) -> AdminFixture {
    let home = seed_institution(pool).await;
    let away = seed_institution(pool).await;
    AdminFixture {
        admin: actor(home, seed_user(pool, home).await, Role::InstitutionAdmin),
        student: actor(home, seed_user(pool, home).await, Role::Student),
        foreign_admin: actor(away, seed_user(pool, away).await, Role::InstitutionAdmin),
    }
}

fn service(pool: &PgPool) -> InstitutionService {
    InstitutionService::new(pool.clone(), crate::audit::AuditWriter)
}

fn day(text: &str) -> NaiveDate {
    text.parse().unwrap()
}

fn event(title: &str, kind: &str, starts: &str, ends: &str) -> EventCommand {
    EventCommand {
        title: title.into(),
        event_type: kind.into(),
        starts_on: day(starts),
        ends_on: day(ends),
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn events_are_admin_only_validated_scoped_and_audited(pool: PgPool) {
    let fx = seed_admin_fixture(&pool).await;
    let service = service(&pool);

    // Only the institution admin manages the calendar.
    let denied = service
        .create_event(
            &fx.student,
            event("Independence Day", "holiday", "2026-09-21", "2026-09-21"),
        )
        .await;
    assert!(matches!(denied, Err(AppError::Forbidden)));

    // Validation: blank title, unknown type, end before start.
    for bad in [
        event("   ", "holiday", "2026-09-21", "2026-09-21"),
        event("Day", "party", "2026-09-21", "2026-09-21"),
        event("Day", "event", "2026-09-22", "2026-09-21"),
    ] {
        let result = service.create_event(&fx.admin, bad).await;
        assert!(matches!(result, Err(AppError::Validation(_))), "{result:?}");
    }

    // A real creation lands with its audit record.
    let event_id = service
        .create_event(
            &fx.admin,
            event("Independence Day", "holiday", "2026-09-21", "2026-09-21"),
        )
        .await
        .unwrap();

    // Duplicate title + start date is a conflict, not a second row.
    let duplicate = service
        .create_event(
            &fx.admin,
            event("Independence Day", "holiday", "2026-09-21", "2026-09-21"),
        )
        .await;
    assert!(matches!(duplicate, Err(AppError::Conflict(_))));

    // The foreign admin can neither see nor delete it.
    assert!(
        service
            .list_events(&fx.foreign_admin)
            .await
            .unwrap()
            .is_empty()
    );
    let foreign_delete = service.delete_event(&fx.foreign_admin, event_id).await;
    assert!(matches!(foreign_delete, Err(AppError::NotFound)));

    // The owner sees it and can remove it.
    let listed = service.list_events(&fx.admin).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].title, "Independence Day");
    service.delete_event(&fx.admin, event_id).await.unwrap();
    assert!(service.list_events(&fx.admin).await.unwrap().is_empty());

    // Both mutations audited, in the owning institution only.
    let audits: Vec<String> = sqlx::query_scalar(
        "SELECT action FROM audit_event WHERE institution_id = $1 \
         AND resource_type = 'institution_event' ORDER BY occurred_at",
    )
    .bind(fx.admin.institution_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        audits,
        vec!["institution.event_created", "institution.event_deleted"]
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn settings_and_document_types_are_admin_only_validated_and_audited(pool: PgPool) {
    let fx = seed_admin_fixture(&pool).await;
    let service = service(&pool);

    // Reads and writes are all institution-admin only.
    assert!(matches!(
        service.settings(&fx.student).await,
        Err(AppError::Forbidden)
    ));
    assert!(matches!(
        service.update_settings(&fx.student, "X", "UTC").await,
        Err(AppError::Forbidden)
    ));
    assert!(matches!(
        service
            .set_document_type_enabled(&fx.student, "signed_document", false)
            .await,
        Err(AppError::Forbidden)
    ));

    // Validation: blank name, timezone PostgreSQL does not know.
    assert!(matches!(
        service.update_settings(&fx.admin, "  ", "UTC").await,
        Err(AppError::Validation(_))
    ));
    assert!(matches!(
        service
            .update_settings(&fx.admin, "Admin U", "Mars/Olympus_Mons")
            .await,
        Err(AppError::Validation(_))
    ));

    // A real update lands, is visible, and is audited.
    service
        .update_settings(&fx.admin, "University of Belize", "America/Belize")
        .await
        .unwrap();
    let settings = service.settings(&fx.admin).await.unwrap();
    assert_eq!(settings.name, "University of Belize");
    assert_eq!(settings.timezone, "America/Belize");

    // The 0015 trigger seeded every institution with all types enabled.
    let types = service.document_type_settings(&fx.admin).await.unwrap();
    assert_eq!(types.len(), 3);
    assert!(types.iter().all(|t| t.enabled));

    // Unknown type is rejected; a real toggle lands and is audited.
    assert!(matches!(
        service
            .set_document_type_enabled(&fx.admin, "diploma", false)
            .await,
        Err(AppError::Validation(_))
    ));
    service
        .set_document_type_enabled(&fx.admin, "signed_document", false)
        .await
        .unwrap();
    let types = service.document_type_settings(&fx.admin).await.unwrap();
    assert!(
        types
            .iter()
            .any(|t| t.document_type == "signed_document" && !t.enabled)
    );

    let audits: Vec<String> = sqlx::query_scalar(
        "SELECT action FROM audit_event WHERE institution_id = $1 \
         AND resource_type = 'institution' ORDER BY occurred_at",
    )
    .bind(fx.admin.institution_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        audits,
        vec![
            "institution.settings_updated",
            "institution.document_type_configured"
        ]
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn disabling_a_document_type_blocks_new_requests_fail_closed(pool: PgPool) {
    let fx = seed_admin_fixture(&pool).await;
    let service = service(&pool);

    // Give the student a profile so document requests are possible at all.
    let student_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO student_profile (id, institution_id, user_id, student_number, program_code) \
         VALUES ($1, $2, $3, $4, 'CS')",
    )
    .bind(student_id)
    .bind(fx.student.institution_id)
    .bind(fx.student.user_id)
    .bind(format!("N-{}", &student_id.to_string()[..8]))
    .execute(&pool)
    .await
    .unwrap();
    let student = Actor {
        student_id: Some(student_id),
        ..fx.student.clone()
    };
    let documents = crate::documents::DocumentService::new(
        pool.clone(),
        crate::audit::AuditWriter,
        crate::records::TranscriptSnapshotService,
    );
    let request = |document_type: &str| crate::documents::RequestDocumentCommand {
        document_type: document_type.into(),
        purpose: None,
        delivery_method: "download".into(),
    };

    // Enabled (the default): the request goes through.
    documents
        .request_for_self(&student, request("enrollment_letter"))
        .await
        .unwrap();

    // Disabled by the admin: the same request is refused...
    service
        .set_document_type_enabled(&fx.admin, "enrollment_letter", false)
        .await
        .unwrap();
    let refused = documents
        .request_for_self(&student, request("enrollment_letter"))
        .await;
    assert!(
        matches!(refused, Err(AppError::Validation(_))),
        "{refused:?}"
    );

    // ...and a missing configuration row fails closed the same way.
    sqlx::query(
        "DELETE FROM institution_document_type \
         WHERE institution_id = $1 AND document_type = 'official_transcript'",
    )
    .bind(fx.admin.institution_id)
    .execute(&pool)
    .await
    .unwrap();
    let refused = documents
        .request_for_self(&student, request("official_transcript"))
        .await;
    assert!(
        matches!(refused, Err(AppError::Validation(_))),
        "{refused:?}"
    );

    // The existing request is untouched by the configuration change.
    let pending: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM document_request WHERE institution_id = $1 AND status = 'pending'",
    )
    .bind(fx.admin.institution_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(pending, 1);
}

/// The prompt's explicit requirement: administration routes must not bypass
/// the domain rules of other features. The admin surfaces call the same
/// services as everyone else, and — proven here, not assumed — the
/// institution_admin role itself carries no power inside enrollment, grades,
/// or documents. Each denial is the SERVICE refusing, so no HTTP route,
/// present or future, can hand an admin a bypass.
#[sqlx::test(migrations = "./migrations")]
async fn institution_admin_does_not_bypass_domain_rules(pool: PgPool) {
    use crate::enrollment::types::EnrollError;
    use crate::records::grades::{CorrectGradeCommand, SaveGradeCommand};

    let fx = seed_admin_fixture(&pool).await;
    let admin = &fx.admin;
    let audit = crate::audit::AuditWriter;

    // Enrollment: an admin can neither register some student nor "self".
    let enrollment = crate::enrollment::EnrollmentService::new(pool.clone(), audit.clone());
    let register = crate::enrollment::RegisterCommand {
        section_id: Uuid::new_v4(),
        idempotency_key: Uuid::new_v4(),
    };
    let for_other = enrollment
        .register_for(
            admin,
            Uuid::new_v4(),
            crate::enrollment::RegisterCommand {
                section_id: register.section_id,
                idempotency_key: Uuid::new_v4(),
            },
        )
        .await;
    assert!(matches!(
        for_other,
        Err(EnrollError::App(AppError::Forbidden))
    ));
    let for_self = enrollment.register_self(admin, register).await;
    assert!(matches!(
        for_self,
        Err(EnrollError::App(AppError::Forbidden))
    ));

    // Grades: draft entry, correction, and publication all refuse the admin.
    let grades = crate::records::GradeService::new(pool.clone(), audit.clone());
    let draft = grades
        .save_draft(
            admin,
            SaveGradeCommand {
                enrollment_id: Uuid::new_v4(),
                grade_code: "A".into(),
                grade_points: Some(4.0),
                numeric_value: None,
                expected_version: 0,
            },
        )
        .await;
    assert!(matches!(draft, Err(AppError::Forbidden)), "{draft:?}");
    let correction = grades
        .correct_grade(
            admin,
            CorrectGradeCommand {
                enrollment_id: Uuid::new_v4(),
                grade_code: "B".into(),
                grade_points: Some(3.0),
                numeric_value: None,
                reason: "admin says so".into(),
                expected_version: 1,
            },
        )
        .await;
    assert!(matches!(correction, Err(AppError::Forbidden)));
    let publish = grades.publish_section(admin, Uuid::new_v4()).await;
    assert!(matches!(publish, Err(AppError::Forbidden)));

    // Documents: deciding requests and downloading artifacts both refuse.
    let documents = crate::documents::DocumentService::new(
        pool.clone(),
        audit.clone(),
        crate::records::TranscriptSnapshotService,
    );
    let approve = documents
        .approve(admin, Uuid::new_v4(), "admin approval")
        .await;
    assert!(matches!(approve, Err(AppError::Forbidden)));
    let reject = documents
        .reject(admin, Uuid::new_v4(), "admin rejection")
        .await;
    assert!(matches!(reject, Err(AppError::Forbidden)));
    let download = documents.downloadable(admin, Uuid::new_v4()).await;
    assert!(matches!(download, Err(AppError::Forbidden)));

    // And nothing above left a trace.
    let enrollments: i64 = sqlx::query_scalar("SELECT count(*) FROM enrollment")
        .fetch_one(&pool)
        .await
        .unwrap();
    let decisions: i64 = sqlx::query_scalar("SELECT count(*) FROM document_approval")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!((enrollments, decisions), (0, 0));
}

mod ui {
    use super::{AdminFixture, seed_admin_fixture};
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

    pub(crate) fn active_gate(institution_id: Uuid) -> LicenseGate {
        LicenseGate::new(LicenseSnapshot {
            institution_id,
            deployment_id: Uuid::new_v4(),
            status: LicenseStatus::Active,
            valid_from: chrono::Utc::now() - chrono::Duration::days(1),
            valid_until: chrono::Utc::now() + chrono::Duration::days(365),
            version: 1,
            feature_set: serde_json::json!({}),
        })
    }

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

    fn extract_input(body: &str, name: &str) -> String {
        let at = body
            .find(&format!("name=\"{name}\""))
            .unwrap_or_else(|| panic!("no input named {name} in page"));
        let rest = &body[at..];
        let value_at = rest.find("value=\"").expect("input has a value") + "value=\"".len();
        rest[value_at..].split('"').next().unwrap().to_owned()
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn calendar_admin_works_as_plain_forms(pool: PgPool) {
        let fx: AdminFixture = seed_admin_fixture(&pool).await;
        let institution = fx.admin.institution_id;
        let admin_login =
            credential(&pool, fx.admin.user_id, institution, "institution_admin").await;
        let student_login = credential(&pool, fx.student.user_id, institution, "student").await;

        let sessions = SessionService::new(pool.clone(), 1800, 43200);
        let auth = AuthService::new(
            pool.clone(),
            PasswordService::new(8, 1, 1).unwrap(),
            sessions.clone(),
            crate::audit::AuditWriter,
            10,
            900,
        )
        .unwrap();
        let app = actix_test::init_service(
            actix_web::App::new()
                .app_data(web::Data::new(sessions))
                .app_data(web::Data::new(auth))
                .app_data(web::Data::new(SessionCookiePolicy {
                    secure: false,
                    max_age_secs: 43200,
                }))
                .app_data(web::Data::new(active_gate(institution)))
                .app_data(web::Data::new(super::service(&pool)))
                .wrap(actix_web::middleware::from_fn(
                    crate::identity_access::csrf::csrf_middleware,
                ))
                .wrap(actix_web::middleware::from_fn(
                    crate::identity_access::middleware::session_middleware,
                ))
                .configure(crate::identity_access::http::routes)
                .configure(crate::institution::http::routes),
        )
        .await;

        let login = |username: String| {
            actix_test::TestRequest::post()
                .uri("/ui/login")
                .peer_addr("127.0.0.1:9999".parse().unwrap())
                .set_form(serde_json::json!({ "username": username, "password": PASSWORD }))
                .to_request()
        };
        let cookie_of = |response: actix_web::dev::ServiceResponse<_>| {
            response
                .response()
                .cookies()
                .find(|cookie| cookie.name() == "ub_session")
                .unwrap()
                .into_owned()
        };
        let admin_cookie = cookie_of(actix_test::call_service(&app, login(admin_login)).await);
        let student_cookie = cookie_of(actix_test::call_service(&app, login(student_login)).await);

        // A student cannot open the admin calendar.
        let denied = actix_test::call_service(
            &app,
            actix_test::TestRequest::get()
                .uri("/ui/admin/calendar")
                .cookie(student_cookie)
                .to_request(),
        )
        .await;
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);

        // Admin: empty page, add through the form, see it, remove it.
        let page = actix_test::call_service(
            &app,
            actix_test::TestRequest::get()
                .uri("/ui/admin/calendar")
                .cookie(admin_cookie.clone())
                .to_request(),
        )
        .await;
        assert_eq!(page.status(), StatusCode::OK);
        let body = String::from_utf8(actix_test::read_body(page).await.to_vec()).unwrap();
        assert!(body.contains("No events yet."));
        let csrf = extract_input(&body, "csrf_token");

        // End-before-start renders inline as 422, nothing stored.
        let invalid = actix_test::call_service(
            &app,
            actix_test::TestRequest::post()
                .uri("/ui/admin/calendar")
                .cookie(admin_cookie.clone())
                .set_form(serde_json::json!({
                    "csrf_token": csrf,
                    "title": "Garifuna Settlement Day",
                    "event_type": "holiday",
                    "starts_on": "2026-11-19",
                    "ends_on": "2026-11-18",
                }))
                .to_request(),
        )
        .await;
        assert_eq!(invalid.status(), StatusCode::UNPROCESSABLE_ENTITY);

        let created = actix_test::call_service(
            &app,
            actix_test::TestRequest::post()
                .uri("/ui/admin/calendar")
                .cookie(admin_cookie.clone())
                .set_form(serde_json::json!({
                    "csrf_token": csrf,
                    "title": "Garifuna Settlement Day",
                    "event_type": "holiday",
                    "starts_on": "2026-11-19",
                    "ends_on": "2026-11-19",
                }))
                .to_request(),
        )
        .await;
        assert_eq!(created.status(), StatusCode::SEE_OTHER);

        let page = actix_test::call_service(
            &app,
            actix_test::TestRequest::get()
                .uri("/ui/admin/calendar")
                .cookie(admin_cookie.clone())
                .to_request(),
        )
        .await;
        let body = String::from_utf8(actix_test::read_body(page).await.to_vec()).unwrap();
        assert!(body.contains("Garifuna Settlement Day"));

        let event_id: Uuid = sqlx::query_scalar("SELECT id FROM institution_event LIMIT 1")
            .fetch_one(&pool)
            .await
            .unwrap();
        let removed = actix_test::call_service(
            &app,
            actix_test::TestRequest::post()
                .uri(&format!("/ui/admin/calendar/{event_id}/delete"))
                .cookie(admin_cookie.clone())
                .set_form(serde_json::json!({ "csrf_token": csrf }))
                .to_request(),
        )
        .await;
        assert_eq!(removed.status(), StatusCode::SEE_OTHER);

        let page = actix_test::call_service(
            &app,
            actix_test::TestRequest::get()
                .uri("/ui/admin/calendar")
                .cookie(admin_cookie)
                .to_request(),
        )
        .await;
        let body = String::from_utf8(actix_test::read_body(page).await.to_vec()).unwrap();
        assert!(body.contains("No events yet."));
    }
}
