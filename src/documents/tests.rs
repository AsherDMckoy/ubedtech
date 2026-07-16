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
