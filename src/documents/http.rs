//! Document HTTP adapters: the student request/track/download page, the
//! officer review queue, and the authorized artifact download. All pages
//! are plain forms (PRG on success, inline errors) — no JavaScript needed.

use crate::identity_access::sessions::CurrentSession;
use crate::shared::{
    actor::{Actor, Role},
    error::AppError,
};
use actix_web::http::StatusCode;
use actix_web::{HttpResponse, get, post, web};
use askama::Template;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use crate::documents::storage::{DocumentStore, FilesystemDocumentStore};
use crate::documents::{DocumentService, RequestDocumentCommand};

/// Download a ready artifact. `downloadable` performs every authorization
/// check (owner or officer, institution scope, ready + current); this
/// adapter only fetches bytes and refuses to serve anything whose checksum
/// no longer matches what was recorded at generation time.
#[get("/ui/documents/{request_id}/download")]
pub async fn download(
    actor: Actor,
    service: web::Data<DocumentService>,
    store: web::Data<FilesystemDocumentStore>,
    request_id: web::Path<Uuid>,
) -> Result<HttpResponse, AppError> {
    let artifact = service
        .downloadable(&actor, request_id.into_inner())
        .await?;

    let bytes = store.read(&artifact.storage_path).await?;
    if Sha256::digest(&bytes).as_slice() != artifact.content_hash.as_slice() {
        return Err(AppError::Integrity(
            "stored artifact does not match its recorded checksum",
        ));
    }

    Ok(HttpResponse::Ok()
        .content_type(artifact.mime_type)
        // Fixed, safe filename — never derived from stored data or paths.
        .insert_header((
            "Content-Disposition",
            "attachment; filename=\"document.pdf\"",
        ))
        .insert_header(("Cache-Control", "private, no-store"))
        .body(bytes))
}

// ---------------------------------------------------------------------------
// Student page: request a document, track statuses, download when ready.
// ---------------------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct StudentRequestRow {
    id: Uuid,
    document_type: String,
    status: String,
    requested_at: chrono::DateTime<chrono::Utc>,
}

struct StudentRequestView {
    id: Uuid,
    label: &'static str,
    status: String,
    requested_at: String,
}

struct AvailableType {
    value: String,
    label: &'static str,
}

#[derive(Template)]
#[template(path = "pages/documents.html")]
struct DocumentsPage<'a> {
    csrf_token: &'a str,
    available_types: Vec<AvailableType>,
    requests: Vec<StudentRequestView>,
    notice: Option<&'a str>,
    error: Option<&'a str>,
}

#[derive(Deserialize)]
pub struct NoticeQuery {
    notice: Option<String>,
}

#[get("/ui/documents")]
pub async fn documents_page(
    actor: Actor,
    current: CurrentSession,
    pool: web::Data<PgPool>,
    query: web::Query<NoticeQuery>,
) -> Result<HttpResponse, AppError> {
    let notice = match query.notice.as_deref() {
        Some("requested") => Some("Your request was submitted."),
        _ => None,
    };
    let body = render_documents(&actor, &current, &pool, notice, None).await?;
    Ok(html(StatusCode::OK, body))
}

#[derive(Deserialize)]
pub struct RequestDocumentForm {
    document_type: String,
    purpose: Option<String>,
    delivery_method: String,
    csrf_token: String,
}

#[post("/ui/documents")]
pub async fn request_form(
    actor: Actor,
    current: CurrentSession,
    service: web::Data<DocumentService>,
    pool: web::Data<PgPool>,
    form: web::Form<RequestDocumentForm>,
) -> Result<HttpResponse, AppError> {
    let _ = &form.csrf_token; // validated by the CSRF middleware

    let outcome = service
        .request_for_self(
            &actor,
            RequestDocumentCommand {
                document_type: form.document_type.clone(),
                purpose: form
                    .purpose
                    .clone()
                    .filter(|value| !value.trim().is_empty()),
                delivery_method: form.delivery_method.clone(),
            },
        )
        .await;

    match outcome {
        Ok(_) => Ok(see_other("/ui/documents?notice=requested")),
        Err(AppError::Validation(message)) => {
            let body = render_documents(&actor, &current, &pool, None, Some(&message)).await?;
            Ok(html(StatusCode::UNPROCESSABLE_ENTITY, body))
        }
        Err(other) => Err(other),
    }
}

async fn render_documents(
    actor: &Actor,
    current: &CurrentSession,
    pool: &PgPool,
    notice: Option<&str>,
    error: Option<&str>,
) -> Result<String, AppError> {
    let student_id = actor.require_student_self()?;
    let rows = sqlx::query_as::<_, StudentRequestRow>(
        r#"
        SELECT id, document_type, status, requested_at
        FROM document_request
        WHERE institution_id = $1 AND student_id = $2
        ORDER BY requested_at DESC
        LIMIT 50
        "#,
    )
    .bind(actor.institution_id)
    .bind(student_id)
    .fetch_all(pool)
    .await?;

    let requests = rows
        .into_iter()
        .map(|row| StudentRequestView {
            id: row.id,
            label: document_type_label(&row.document_type),
            status: row.status,
            requested_at: row.requested_at.format("%Y-%m-%d %H:%M UTC").to_string(),
        })
        .collect();

    // The form offers only what the institution has enabled; the service
    // re-checks on submit, so this is presentation, not the gate.
    let available_types = sqlx::query_scalar::<_, String>(
        r#"
        SELECT document_type FROM institution_document_type
        WHERE institution_id = $1 AND enabled
        ORDER BY document_type
        "#,
    )
    .bind(actor.institution_id)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|value| AvailableType {
        label: document_type_label(&value),
        value,
    })
    .collect();

    Ok(DocumentsPage {
        csrf_token: &current.csrf_token,
        available_types,
        requests,
        notice,
        error,
    }
    .render()?)
}

// ---------------------------------------------------------------------------
// Officer queue: review, approve (reasoned), reject (reasoned).
// ---------------------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct PendingRequestRow {
    id: Uuid,
    document_type: String,
    student_number: String,
    purpose: Option<String>,
    requested_at: chrono::DateTime<chrono::Utc>,
}

struct PendingRequestView {
    id: Uuid,
    label: &'static str,
    student_number: String,
    purpose: String,
    requested_at: String,
}

#[derive(Template)]
#[template(path = "pages/document_queue.html")]
struct QueuePage<'a> {
    csrf_token: &'a str,
    requests: Vec<PendingRequestView>,
    notice: Option<&'a str>,
    error: Option<&'a str>,
}

#[get("/ui/admin/documents")]
pub async fn queue_page(
    actor: Actor,
    current: CurrentSession,
    pool: web::Data<PgPool>,
    query: web::Query<NoticeQuery>,
) -> Result<HttpResponse, AppError> {
    let notice = match query.notice.as_deref() {
        Some("approved") => Some("Request approved; generation queued."),
        Some("rejected") => Some("Request rejected."),
        _ => None,
    };
    let body = render_queue(&actor, &current, &pool, notice, None).await?;
    Ok(html(StatusCode::OK, body))
}

#[derive(Deserialize)]
pub struct DecisionForm {
    note: Option<String>,
    csrf_token: String,
}

#[post("/ui/admin/documents/{request_id}/approve")]
pub async fn approve_form(
    actor: Actor,
    current: CurrentSession,
    service: web::Data<DocumentService>,
    pool: web::Data<PgPool>,
    request_id: web::Path<Uuid>,
    form: web::Form<DecisionForm>,
) -> Result<HttpResponse, AppError> {
    let _ = &form.csrf_token;
    let outcome = service
        .approve(
            &actor,
            request_id.into_inner(),
            form.note.as_deref().unwrap_or(""),
        )
        .await;
    finish_decision(&actor, &current, &pool, outcome, "approved").await
}

#[post("/ui/admin/documents/{request_id}/reject")]
pub async fn reject_form(
    actor: Actor,
    current: CurrentSession,
    service: web::Data<DocumentService>,
    pool: web::Data<PgPool>,
    request_id: web::Path<Uuid>,
    form: web::Form<DecisionForm>,
) -> Result<HttpResponse, AppError> {
    let _ = &form.csrf_token;
    let outcome = service
        .reject(
            &actor,
            request_id.into_inner(),
            form.note.as_deref().unwrap_or(""),
        )
        .await;
    finish_decision(&actor, &current, &pool, outcome, "rejected").await
}

/// PRG on success; validation problems and already-decided races render
/// inline on the queue so the officer never loses their place.
async fn finish_decision(
    actor: &Actor,
    current: &CurrentSession,
    pool: &PgPool,
    outcome: Result<(), AppError>,
    notice: &str,
) -> Result<HttpResponse, AppError> {
    match outcome {
        Ok(()) => Ok(see_other(&format!("/ui/admin/documents?notice={notice}"))),
        Err(AppError::Validation(message)) => {
            let body = render_queue(actor, current, pool, None, Some(&message)).await?;
            Ok(html(StatusCode::UNPROCESSABLE_ENTITY, body))
        }
        Err(AppError::NotFound) => {
            let body = render_queue(
                actor,
                current,
                pool,
                None,
                Some("that request is no longer pending"),
            )
            .await?;
            Ok(html(StatusCode::NOT_FOUND, body))
        }
        Err(other) => Err(other),
    }
}

async fn render_queue(
    actor: &Actor,
    current: &CurrentSession,
    pool: &PgPool,
    notice: Option<&str>,
    error: Option<&str>,
) -> Result<String, AppError> {
    // Reading the queue is officer-only, like deciding it.
    if !actor.has_role(Role::DocumentOfficer) {
        return Err(AppError::Forbidden);
    }

    let rows = sqlx::query_as::<_, PendingRequestRow>(
        r#"
        SELECT
            dr.id,
            dr.document_type,
            sp.student_number,
            dr.purpose,
            dr.requested_at
        FROM document_request dr
        JOIN student_profile sp ON sp.id = dr.student_id
        WHERE dr.institution_id = $1 AND dr.status = 'pending'
        ORDER BY dr.requested_at
        LIMIT 100
        "#,
    )
    .bind(actor.institution_id)
    .fetch_all(pool)
    .await?;

    let requests = rows
        .into_iter()
        .map(|row| PendingRequestView {
            id: row.id,
            label: document_type_label(&row.document_type),
            student_number: row.student_number,
            purpose: row.purpose.unwrap_or_default(),
            requested_at: row.requested_at.format("%Y-%m-%d %H:%M UTC").to_string(),
        })
        .collect();

    Ok(QueuePage {
        csrf_token: &current.csrf_token,
        requests,
        notice,
        error,
    }
    .render()?)
}

fn document_type_label(value: &str) -> &'static str {
    match value {
        "official_transcript" => "Official transcript",
        "enrollment_letter" => "Proof of enrollment",
        "signed_document" => "Signed document",
        _ => "Document",
    }
}

fn html(status: StatusCode, body: String) -> HttpResponse {
    HttpResponse::build(status)
        .content_type("text/html; charset=utf-8")
        .body(body)
}

fn see_other(location: &str) -> HttpResponse {
    HttpResponse::SeeOther()
        .insert_header(("Location", location.to_owned()))
        .finish()
}

pub fn routes(cfg: &mut web::ServiceConfig) {
    cfg.service(documents_page)
        .service(request_form)
        .service(download)
        .service(queue_page)
        .service(approve_form)
        .service(reject_form);
}
