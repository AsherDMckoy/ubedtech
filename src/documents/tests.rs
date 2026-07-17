//! Documents integration tests: the request→approve→generate workflow, the
//! orphaned-job reaper (CLAUDE.md §1 item 3), bounded retries, idempotent
//! generation, and download authorization.

use sqlx::PgPool;
use std::collections::HashSet;
use uuid::Uuid;

use crate::documents::{DocumentService, DocumentWorker, RequestDocumentCommand};
use crate::shared::actor::{Actor, Role};

pub(super) struct DocFixture {
    pub student: Actor,
    pub officer: Actor,
}

fn actor(institution_id: Uuid, user_id: Uuid, role: Role, student_id: Option<Uuid>) -> Actor {
    Actor {
        user_id,
        institution_id,
        student_id,
        roles: HashSet::from([role]),
    }
}

pub(super) async fn seed_doc_fixture(pool: &PgPool) -> DocFixture {
    let institution_id = Uuid::new_v4();
    sqlx::query("INSERT INTO institution (id, code, name) VALUES ($1, $2, 'Docs U')")
        .bind(institution_id)
        .bind(format!("D-{}", &institution_id.to_string()[..8]))
        .execute(pool)
        .await
        .unwrap();

    let mut users = Vec::new();
    for _ in 0..2 {
        let user_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO user_account (id, institution_id, username, email) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(user_id)
        .bind(institution_id)
        .bind(format!("u-{}", &user_id.to_string()[..12]))
        .bind(format!("{}@test.invalid", &user_id.to_string()[..12]))
        .execute(pool)
        .await
        .unwrap();
        users.push(user_id);
    }

    let student_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO student_profile (id, institution_id, user_id, student_number, program_code) \
         VALUES ($1, $2, $3, $4, 'CS')",
    )
    .bind(student_id)
    .bind(institution_id)
    .bind(users[0])
    .bind(format!("N-{}", &student_id.to_string()[..8]))
    .execute(pool)
    .await
    .unwrap();

    DocFixture {
        student: actor(institution_id, users[0], Role::Student, Some(student_id)),
        officer: actor(institution_id, users[1], Role::DocumentOfficer, None),
    }
}

pub(super) fn doc_service(pool: &PgPool) -> DocumentService {
    DocumentService::new(
        pool.clone(),
        crate::audit::AuditWriter,
        crate::records::TranscriptSnapshotService,
    )
}

pub(super) fn worker(pool: &PgPool) -> DocumentWorker {
    let root = std::env::temp_dir().join(format!("ubed-doc-test-{}", Uuid::new_v4()));
    DocumentWorker::new(pool.clone(), "test-worker".into(), root, 60)
}

/// Request + approve an official transcript; returns (request_id, job_id).
pub(super) async fn approved_request(pool: &PgPool, fx: &DocFixture) -> (Uuid, Uuid) {
    let service = doc_service(pool);
    let receipt = service
        .request_for_self(
            &fx.student,
            RequestDocumentCommand {
                document_type: "official_transcript".into(),
                purpose: Some("scholarship".into()),
                delivery_method: "download".into(),
                idempotency_key: Uuid::new_v4(),
            },
        )
        .await
        .unwrap();
    service
        .approve(&fx.officer, receipt.request_id, "identity verified")
        .await
        .unwrap();
    let job_id: Uuid = sqlx::query_scalar("SELECT id FROM document_job WHERE request_id = $1")
        .bind(receipt.request_id)
        .fetch_one(pool)
        .await
        .unwrap();
    (receipt.request_id, job_id)
}

async fn job_state(pool: &PgPool, job_id: Uuid) -> (String, i32, Option<String>) {
    sqlx::query_as("SELECT status, attempts, last_error FROM document_job WHERE id = $1")
        .bind(job_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn request_status(pool: &PgPool, request_id: Uuid) -> String {
    sqlx::query_scalar("SELECT status FROM document_request WHERE id = $1")
        .bind(request_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// CLAUDE.md §1 item 3, the required proof: a worker dies mid-render (the
/// 'running' state is committed, nothing ever finishes it); the reaper
/// returns the job to 'queued' using locked_at/locked_by, and a live worker
/// then completes it.
#[sqlx::test(migrations = "./migrations")]
async fn a_crashed_workers_job_is_reaped_and_completed_by_a_live_worker(pool: PgPool) {
    let fx = seed_doc_fixture(&pool).await;
    let (request_id, job_id) = approved_request(&pool, &fx).await;
    let worker = worker(&pool);

    // Simulate the crash exactly as it happens in production: the claim
    // transaction committed (running + locked_at + locked_by + attempt
    // spent, request 'generating'), then the process died mid-render.
    sqlx::query(
        "UPDATE document_job SET status = 'running', attempts = 1, \
         locked_at = now() - interval '1 hour', locked_by = 'worker-that-died' WHERE id = $1",
    )
    .bind(job_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE document_request SET status = 'generating' WHERE id = $1")
        .bind(request_id)
        .execute(&pool)
        .await
        .unwrap();

    // A fresh 'running' job (healthy worker, mid-render right now) must NOT
    // be reaped — only ones past the stale threshold.
    let reaped = worker.reap_stale().await.unwrap();
    assert_eq!(reaped, 1);

    let (status, attempts, last_error) = job_state(&pool, job_id).await;
    assert_eq!(status, "queued", "the orphaned job is claimable again");
    assert_eq!(attempts, 1, "the dead worker's attempt stays spent");
    assert!(last_error.unwrap().contains("worker-that-died"));
    let (locked_at, locked_by): (Option<chrono::DateTime<chrono::Utc>>, Option<String>) =
        sqlx::query_as("SELECT locked_at, locked_by FROM document_job WHERE id = $1")
            .bind(job_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(locked_at.is_none() && locked_by.is_none());

    // A live worker picks it up and finishes the request.
    assert!(worker.run_once().await.unwrap());
    let (status, _, _) = job_state(&pool, job_id).await;
    assert_eq!(status, "complete");
    assert_eq!(request_status(&pool, request_id).await, "ready");
    let artifacts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM generated_document WHERE request_id = $1 \
         AND superseded_at IS NULL",
    )
    .bind(request_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(artifacts, 1);
}

/// A job that keeps dying stops retrying: once the attempt budget is spent,
/// the reaper fails it terminally and the request reports failure honestly.
#[sqlx::test(migrations = "./migrations")]
async fn reaping_past_the_attempt_budget_fails_terminally(pool: PgPool) {
    let fx = seed_doc_fixture(&pool).await;
    let (request_id, job_id) = approved_request(&pool, &fx).await;
    let worker = worker(&pool);

    sqlx::query(
        "UPDATE document_job SET status = 'running', attempts = 3, \
         locked_at = now() - interval '1 hour', locked_by = 'crashloop' WHERE id = $1",
    )
    .bind(job_id)
    .execute(&pool)
    .await
    .unwrap();

    assert_eq!(worker.reap_stale().await.unwrap(), 1);
    let (status, _, last_error) = job_state(&pool, job_id).await;
    assert_eq!(status, "failed");
    assert!(last_error.unwrap().contains("reaped"));
    assert_eq!(request_status(&pool, request_id).await, "failed");

    // Nothing left to reap or run.
    assert_eq!(worker.reap_stale().await.unwrap(), 0);
    assert!(!worker.run_once().await.unwrap());
}

/// A healthy running job inside the stale window is left alone.
#[sqlx::test(migrations = "./migrations")]
async fn the_reaper_leaves_live_jobs_alone(pool: PgPool) {
    let fx = seed_doc_fixture(&pool).await;
    let (_, job_id) = approved_request(&pool, &fx).await;
    let worker = worker(&pool);

    sqlx::query(
        "UPDATE document_job SET status = 'running', attempts = 1, \
         locked_at = now(), locked_by = 'healthy-worker' WHERE id = $1",
    )
    .bind(job_id)
    .execute(&pool)
    .await
    .unwrap();

    assert_eq!(worker.reap_stale().await.unwrap(), 0);
    let (status, _, _) = job_state(&pool, job_id).await;
    assert_eq!(status, "running");
}

/// Resubmitted generation converges: a duplicate job for an already-ready
/// request completes without producing a second artifact.
#[sqlx::test(migrations = "./migrations")]
async fn duplicate_jobs_never_produce_a_second_artifact(pool: PgPool) {
    let fx = seed_doc_fixture(&pool).await;
    let (request_id, _) = approved_request(&pool, &fx).await;
    let worker = worker(&pool);

    assert!(worker.run_once().await.unwrap());
    assert_eq!(request_status(&pool, request_id).await, "ready");

    // A duplicate job (crash between artifact insert and job completion, or
    // an operator requeue) runs to completion but keeps the one artifact.
    let duplicate_job = Uuid::new_v4();
    sqlx::query("INSERT INTO document_job (id, request_id, status) VALUES ($1, $2, 'queued')")
        .bind(duplicate_job)
        .bind(request_id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(worker.run_once().await.unwrap());

    let (status, _, _) = job_state(&pool, duplicate_job).await;
    assert_eq!(status, "complete");
    assert_eq!(request_status(&pool, request_id).await, "ready");
    let artifacts: i64 =
        sqlx::query_scalar("SELECT count(*) FROM generated_document WHERE request_id = $1")
            .bind(request_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(artifacts, 1, "exactly one artifact, ever");
}

/// Render failures retry with recorded reasons and a hard cap. (A
/// signed_document request has no snapshot, which the demo renderer
/// refuses — a deterministic failure.)
#[sqlx::test(migrations = "./migrations")]
async fn failed_renders_retry_with_recorded_reasons_then_stop(pool: PgPool) {
    let fx = seed_doc_fixture(&pool).await;
    let service = doc_service(&pool);
    let receipt = service
        .request_for_self(
            &fx.student,
            RequestDocumentCommand {
                document_type: "signed_document".into(),
                purpose: None,
                delivery_method: "pickup".into(),
                idempotency_key: Uuid::new_v4(),
            },
        )
        .await
        .unwrap();
    service
        .approve(&fx.officer, receipt.request_id, "verified")
        .await
        .unwrap();
    let job_id: Uuid = sqlx::query_scalar("SELECT id FROM document_job WHERE request_id = $1")
        .bind(receipt.request_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let worker = worker(&pool);

    for attempt in 1..=3 {
        assert!(worker.run_once().await.unwrap());
        let (status, attempts, last_error) = job_state(&pool, job_id).await;
        assert_eq!(attempts, attempt);
        assert!(last_error.is_some(), "failure reason recorded");
        if attempt < 3 {
            assert_eq!(status, "queued", "bounded retry, attempt {attempt}");
            // Pull the backoff forward so the next run_once sees the job.
            sqlx::query("UPDATE document_job SET available_at = now() WHERE id = $1")
                .bind(job_id)
                .execute(&pool)
                .await
                .unwrap();
        } else {
            assert_eq!(status, "failed", "attempt budget exhausted");
        }
    }

    assert_eq!(request_status(&pool, receipt.request_id).await, "failed");
    let artifacts: i64 = sqlx::query_scalar("SELECT count(*) FROM generated_document")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(artifacts, 0, "no fake success artifact");
}

/// Downloads: the owning student and the document officer succeed
/// (institution-scoped); everyone else — another student, a foreign
/// officer, a role with no student profile — gets 404/403. Bytes are a
/// real PDF matching the recorded checksum, and a not-yet-ready request
/// serves nothing.
#[sqlx::test(migrations = "./migrations")]
async fn downloads_are_authorized_and_checksum_verified(pool: PgPool) {
    use crate::documents::storage::DocumentStore;
    use crate::shared::error::AppError;
    use sha2::{Digest, Sha256};

    let fx = seed_doc_fixture(&pool).await;
    let service = doc_service(&pool);

    // Not ready yet: even the owner gets 404 before generation.
    let (request_id, _) = approved_request(&pool, &fx).await;
    assert!(matches!(
        service.downloadable(&fx.student, request_id).await,
        Err(AppError::NotFound)
    ));

    let root = std::env::temp_dir().join(format!("ubed-doc-test-{}", Uuid::new_v4()));
    let worker = DocumentWorker::new(pool.clone(), "test-worker".into(), root.clone(), 60);
    assert!(worker.run_once().await.unwrap());

    // Owner and officer both resolve the artifact; the bytes are a PDF and
    // match the recorded checksum.
    for who in [&fx.student, &fx.officer] {
        let artifact = service.downloadable(who, request_id).await.unwrap();
        let store = crate::documents::storage::FilesystemDocumentStore::new(root.clone());
        let bytes = store.read(&artifact.storage_path).await.unwrap();
        assert!(bytes.starts_with(b"%PDF-"));
        assert_eq!(Sha256::digest(&bytes).as_slice(), artifact.content_hash);
    }

    // Another student in the same institution: 404, not someone else's
    // transcript.
    let other_user = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO user_account (id, institution_id, username, email) VALUES ($1, $2, $3, $4)",
    )
    .bind(other_user)
    .bind(fx.student.institution_id)
    .bind(format!("o-{}", &other_user.to_string()[..12]))
    .bind(format!("{}@test.invalid", &other_user.to_string()[..12]))
    .execute(&pool)
    .await
    .unwrap();
    let other_student_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO student_profile (id, institution_id, user_id, student_number, program_code) \
         VALUES ($1, $2, $3, $4, 'CS')",
    )
    .bind(other_student_id)
    .bind(fx.student.institution_id)
    .bind(other_user)
    .bind(format!("X-{}", &other_student_id.to_string()[..8]))
    .execute(&pool)
    .await
    .unwrap();
    let other_student = actor(
        fx.student.institution_id,
        other_user,
        Role::Student,
        Some(other_student_id),
    );
    assert!(matches!(
        service.downloadable(&other_student, request_id).await,
        Err(AppError::NotFound)
    ));

    // A foreign officer and a profileless role get nothing either.
    let foreign_institution = Uuid::new_v4();
    sqlx::query("INSERT INTO institution (id, code, name) VALUES ($1, $2, 'F U')")
        .bind(foreign_institution)
        .bind(format!("F-{}", &foreign_institution.to_string()[..8]))
        .execute(&pool)
        .await
        .unwrap();
    let foreign_officer = actor(
        foreign_institution,
        Uuid::new_v4(),
        Role::DocumentOfficer,
        None,
    );
    assert!(matches!(
        service.downloadable(&foreign_officer, request_id).await,
        Err(AppError::NotFound)
    ));
    let instructor = actor(
        fx.student.institution_id,
        Uuid::new_v4(),
        Role::Instructor,
        None,
    );
    assert!(matches!(
        service.downloadable(&instructor, request_id).await,
        Err(AppError::Forbidden)
    ));
}

/// Approval: officer-only, reasoned, institution-scoped; the immutable
/// snapshot and the generation job land in the SAME transaction as the
/// approved state. Rejection: reasoned and recorded.
#[sqlx::test(migrations = "./migrations")]
async fn approval_and_rejection_are_reasoned_scoped_and_atomic(pool: PgPool) {
    use crate::documents::RequestDocumentCommand;
    use crate::shared::error::AppError;

    let fx = seed_doc_fixture(&pool).await;
    let service = doc_service(&pool);
    let request = |purpose: &str| {
        service.request_for_self(
            &fx.student,
            RequestDocumentCommand {
                document_type: "official_transcript".into(),
                purpose: Some(purpose.into()),
                delivery_method: "download".into(),
                idempotency_key: Uuid::new_v4(),
            },
        )
    };
    let first = request("approval test").await.unwrap().request_id;

    // Authorization and validation, none of which may leave any state.
    assert!(matches!(
        service.approve(&fx.student, first, "self-approval").await,
        Err(AppError::Forbidden)
    ));
    assert!(matches!(
        service.approve(&fx.officer, first, "   ").await,
        Err(AppError::Validation(_))
    ));
    let foreign_institution = Uuid::new_v4();
    sqlx::query("INSERT INTO institution (id, code, name) VALUES ($1, $2, 'F U')")
        .bind(foreign_institution)
        .bind(format!("F-{}", &foreign_institution.to_string()[..8]))
        .execute(&pool)
        .await
        .unwrap();
    let foreign_officer = actor(
        foreign_institution,
        Uuid::new_v4(),
        Role::DocumentOfficer,
        None,
    );
    assert!(matches!(
        service.approve(&foreign_officer, first, "not yours").await,
        Err(AppError::NotFound)
    ));
    let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM document_job")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(jobs, 0, "denied approvals queued nothing");

    // The real approval: approved state, snapshot, and queued job are all
    // present after the one commit — an approved request cannot exist
    // without durable work to produce its artifact.
    service
        .approve(&fx.officer, first, "identity verified at counter")
        .await
        .unwrap();
    let (status, snapshot_id): (String, Option<Uuid>) =
        sqlx::query_as("SELECT status, current_snapshot_id FROM document_request WHERE id = $1")
            .bind(first)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "approved");
    let snapshot_id = snapshot_id.expect("snapshot captured at approval time");
    let snapshot_exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM transcript_snapshot WHERE id = $1)")
            .bind(snapshot_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(snapshot_exists);
    let queued: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM document_job WHERE request_id = $1 AND status = 'queued'",
    )
    .bind(first)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(queued, 1);

    // Approving twice: the request is no longer pending, so 404 — and no
    // second job or snapshot appears.
    assert!(matches!(
        service.approve(&fx.officer, first, "again").await,
        Err(AppError::NotFound)
    ));

    // Rejection requires a reason and records the decision.
    let second = request("rejection test").await.unwrap().request_id;
    assert!(matches!(
        service.reject(&fx.officer, second, "  ").await,
        Err(AppError::Validation(_))
    ));
    service
        .reject(&fx.officer, second, "identity mismatch")
        .await
        .unwrap();
    assert_eq!(request_status(&pool, second).await, "rejected");
    let (decision, note): (String, Option<String>) =
        sqlx::query_as("SELECT decision, note FROM document_approval WHERE request_id = $1")
            .bind(second)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(decision, "rejected");
    assert_eq!(note.as_deref(), Some("identity mismatch"));

    // Both decisions audited.
    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_event WHERE action IN \
         ('document.approved', 'document.rejected')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(audits, 2);
}

// ---------------------------------------------------------------------------
// Pages over plain forms: request → review → approve → generate → download.
// ---------------------------------------------------------------------------

mod ui {
    use super::{DocFixture, seed_doc_fixture};
    use actix_web::http::StatusCode;
    use actix_web::{test as actix_test, web};
    use sqlx::PgPool;
    use uuid::Uuid;

    use crate::documents::DocumentWorker;
    use crate::documents::storage::FilesystemDocumentStore;
    use crate::identity_access::http::SessionCookiePolicy;
    use crate::identity_access::password::PasswordService;
    use crate::identity_access::service::AuthService;
    use crate::identity_access::sessions::SessionService;
    use crate::licensing::{LicenseGate, LicenseSnapshot, LicenseStatus};

    const PASSWORD: &str = "correct horse battery";

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
    async fn request_review_generate_download_works_as_plain_forms(pool: PgPool) {
        let fx: DocFixture = seed_doc_fixture(&pool).await;
        let institution = fx.officer.institution_id;
        let student_login = credential(&pool, fx.student.user_id, institution, "student").await;
        let officer_login =
            credential(&pool, fx.officer.user_id, institution, "document_officer").await;

        // One storage root shared by the app's download path and the worker.
        let root = std::env::temp_dir().join(format!("ubed-doc-ui-{}", Uuid::new_v4()));
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
        let gate = LicenseGate::new(LicenseSnapshot {
            institution_id: institution,
            deployment_id: Uuid::new_v4(),
            status: LicenseStatus::Active,
            valid_from: chrono::Utc::now() - chrono::Duration::days(1),
            valid_until: chrono::Utc::now() + chrono::Duration::days(365),
            version: 1,
            feature_set: serde_json::json!({}),
        });
        let app = actix_test::init_service(
            actix_web::App::new()
                .app_data(web::Data::new(sessions))
                .app_data(web::Data::new(auth))
                .app_data(web::Data::new(gate))
                .app_data(web::Data::new(SessionCookiePolicy {
                    secure: false,
                    max_age_secs: 43200,
                }))
                .app_data(web::Data::new(pool.clone()))
                .app_data(web::Data::new(super::doc_service(&pool)))
                .app_data(web::Data::new(FilesystemDocumentStore::new(root.clone())))
                .wrap(actix_web::middleware::from_fn(
                    crate::identity_access::csrf::csrf_middleware,
                ))
                .wrap(actix_web::middleware::from_fn(
                    crate::identity_access::middleware::session_middleware,
                ))
                .configure(crate::identity_access::http::routes)
                .configure(crate::documents::http::routes),
        )
        .await;

        let login = |username: String| {
            actix_test::TestRequest::post()
                .uri("/ui/login")
                .peer_addr("127.0.0.1:9999".parse().unwrap())
                .set_form(serde_json::json!({ "username": username, "password": PASSWORD }))
                .to_request()
        };
        let student_response = actix_test::call_service(&app, login(student_login)).await;
        let student_cookie = student_response
            .response()
            .cookies()
            .find(|cookie| cookie.name() == "ub_session")
            .unwrap()
            .into_owned();
        let officer_response = actix_test::call_service(&app, login(officer_login)).await;
        let officer_cookie = officer_response
            .response()
            .cookies()
            .find(|cookie| cookie.name() == "ub_session")
            .unwrap()
            .into_owned();

        // Student requests through the form.
        let page = actix_test::call_service(
            &app,
            actix_test::TestRequest::get()
                .uri("/ui/documents")
                .cookie(student_cookie.clone())
                .to_request(),
        )
        .await;
        assert_eq!(page.status(), StatusCode::OK);
        let body = String::from_utf8(actix_test::read_body(page).await.to_vec()).unwrap();
        assert!(body.contains("No document requests yet."));
        let student_csrf = extract_input(&body, "csrf_token");
        // The rendered form carries a server-minted idempotency key.
        let form_key = extract_input(&body, "idempotency_key");
        let submit = || {
            actix_test::TestRequest::post()
                .uri("/ui/documents")
                .cookie(student_cookie.clone())
                .set_form(serde_json::json!({
                    "csrf_token": student_csrf,
                    "idempotency_key": form_key,
                    "document_type": "official_transcript",
                    "purpose": "visa application",
                    "delivery_method": "download",
                }))
                .to_request()
        };
        let response = actix_test::call_service(&app, submit()).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);

        // Resubmitting the same rendered form (double click, retry) does
        // not file a second request.
        let response = actix_test::call_service(&app, submit()).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM document_request")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            count, 1,
            "duplicate submission must not create a second request"
        );

        // Students cannot open the officer queue.
        let response = actix_test::call_service(
            &app,
            actix_test::TestRequest::get()
                .uri("/ui/admin/documents")
                .cookie(student_cookie.clone())
                .to_request(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        // Officer sees the request; a blank approval reason renders inline.
        let queue = actix_test::call_service(
            &app,
            actix_test::TestRequest::get()
                .uri("/ui/admin/documents")
                .cookie(officer_cookie.clone())
                .to_request(),
        )
        .await;
        let queue_body = String::from_utf8(actix_test::read_body(queue).await.to_vec()).unwrap();
        assert!(queue_body.contains("Official transcript"));
        assert!(queue_body.contains("visa application"));
        let officer_csrf = extract_input(&queue_body, "csrf_token");
        let request_id: uuid::Uuid = sqlx::query_scalar("SELECT id FROM document_request LIMIT 1")
            .fetch_one(&pool)
            .await
            .unwrap();

        let response = actix_test::call_service(
            &app,
            actix_test::TestRequest::post()
                .uri(&format!("/ui/admin/documents/{request_id}/approve"))
                .cookie(officer_cookie.clone())
                .set_form(serde_json::json!({ "csrf_token": officer_csrf, "note": "  " }))
                .to_request(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = String::from_utf8(actix_test::read_body(response).await.to_vec()).unwrap();
        assert!(body.contains("approval reason is required"));

        let response = actix_test::call_service(
            &app,
            actix_test::TestRequest::post()
                .uri(&format!("/ui/admin/documents/{request_id}/approve"))
                .cookie(officer_cookie.clone())
                .set_form(serde_json::json!({
                    "csrf_token": officer_csrf,
                    "note": "identity verified",
                }))
                .to_request(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);

        // The worker (same storage root) generates the artifact.
        let worker = DocumentWorker::new(pool.clone(), "ui-test-worker".into(), root.clone(), 60);
        assert!(worker.run_once().await.unwrap());

        // Student's page now shows ready + a download link, and the download
        // itself is an attachment with PDF bytes.
        let page = actix_test::call_service(
            &app,
            actix_test::TestRequest::get()
                .uri("/ui/documents")
                .cookie(student_cookie.clone())
                .to_request(),
        )
        .await;
        let body = String::from_utf8(actix_test::read_body(page).await.to_vec()).unwrap();
        assert!(body.contains("ready"));
        assert!(body.contains(&format!("/ui/documents/{request_id}/download")));

        let response = actix_test::call_service(
            &app,
            actix_test::TestRequest::get()
                .uri(&format!("/ui/documents/{request_id}/download"))
                .cookie(student_cookie.clone())
                .to_request(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            response
                .headers()
                .get("Content-Disposition")
                .unwrap()
                .to_str()
                .unwrap()
                .contains("attachment")
        );
        let bytes = actix_test::read_body(response).await;
        assert!(bytes.starts_with(b"%PDF-"));
    }
}

/// FOR UPDATE SKIP LOCKED under real concurrency: two workers race one
/// queued job; exactly one claims and completes it, the other finds nothing.
#[sqlx::test(migrations = "./migrations")]
async fn two_workers_cannot_claim_the_same_job(pool: PgPool) {
    let fx = seed_doc_fixture(&pool).await;
    let (request_id, _) = approved_request(&pool, &fx).await;

    let left_worker = worker(&pool);
    let right_worker = worker(&pool);
    let (left, right) = tokio::join!(left_worker.run_once(), right_worker.run_once());
    let claims = usize::from(left.unwrap()) + usize::from(right.unwrap());
    assert_eq!(claims, 1, "exactly one worker claimed the job");

    assert_eq!(request_status(&pool, request_id).await, "ready");
    let artifacts: i64 =
        sqlx::query_scalar("SELECT count(*) FROM generated_document WHERE request_id = $1")
            .bind(request_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(artifacts, 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn duplicate_document_requests_with_one_key_return_the_original(pool: PgPool) {
    let fx = seed_doc_fixture(&pool).await;
    let service = doc_service(&pool);
    let key = Uuid::new_v4();
    let command = || RequestDocumentCommand {
        document_type: "official_transcript".into(),
        purpose: None,
        delivery_method: "download".into(),
        idempotency_key: key,
    };

    // Two concurrent submissions of the same key: one row, one id.
    let (first, second) = tokio::join!(
        service.request_for_self(&fx.student, command()),
        service.request_for_self(&fx.student, command()),
    );
    let first = first.unwrap();
    let second = second.unwrap();
    assert_eq!(first.request_id, second.request_id);

    // A later resubmission still returns the original, with its current
    // status rather than a fake fresh "pending".
    let again = service
        .request_for_self(&fx.student, command())
        .await
        .unwrap();
    assert_eq!(again.request_id, first.request_id);
    assert_eq!(again.status, "pending");

    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM document_request")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);

    // A different key is a genuinely new request.
    let other = service
        .request_for_self(
            &fx.student,
            RequestDocumentCommand {
                idempotency_key: Uuid::new_v4(),
                ..command()
            },
        )
        .await
        .unwrap();
    assert_ne!(other.request_id, first.request_id);
}
