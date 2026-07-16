use crate::shared::{
    actor::{Actor, Role},
    error::AppError,
};
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

#[derive(Deserialize)]
pub struct RequestDocumentForm {
    document_type: String,
    purpose: Option<String>,
    delivery_method: String,
    csrf_token: String,
}

#[post("/ui/document-requests")]
pub async fn request_fragment(
    actor: Actor,
    service: web::Data<DocumentService>,
    pool: web::Data<PgPool>,
    form: web::Form<RequestDocumentForm>,
) -> Result<HttpResponse, AppError> {
    let _ = &form.csrf_token; // Validated by CSRF middleware.

    service
        .request_for_self(
            &actor,
            RequestDocumentCommand {
                document_type: form.document_type.clone(),
                purpose: form.purpose.clone(),
                delivery_method: form.delivery_method.clone(),
            },
        )
        .await?;

    Ok(HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(render_student_request_list(&actor, &pool).await?))
}

#[derive(Deserialize)]
pub struct DecisionForm {
    note: Option<String>,
    csrf_token: String,
}

#[post("/ui/admin/document-requests/{request_id}/approve")]
pub async fn approve_fragment(
    actor: Actor,
    service: web::Data<DocumentService>,
    pool: web::Data<PgPool>,
    request_id: web::Path<Uuid>,
    form: web::Form<DecisionForm>,
) -> Result<HttpResponse, AppError> {
    let _ = &form.csrf_token;
    service
        .approve(
            &actor,
            request_id.into_inner(),
            form.note.as_deref().unwrap_or(""),
        )
        .await?;

    Ok(HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(render_admin_queue(&actor, &pool, &form.csrf_token).await?))
}

#[post("/ui/admin/document-requests/{request_id}/reject")]
pub async fn reject_fragment(
    actor: Actor,
    service: web::Data<DocumentService>,
    pool: web::Data<PgPool>,
    request_id: web::Path<Uuid>,
    form: web::Form<DecisionForm>,
) -> Result<HttpResponse, AppError> {
    let note = form.note.as_deref().unwrap_or_default();
    service
        .reject(&actor, request_id.into_inner(), note)
        .await?;

    Ok(HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(render_admin_queue(&actor, &pool, &form.csrf_token).await?))
}

#[derive(sqlx::FromRow)]
struct StudentRequestRow {
    document_type: String,
    status: String,
    requested_at: chrono::DateTime<chrono::Utc>,
}

struct StudentRequestView {
    document_type_label: &'static str,
    status: String,
    requested_at: String,
}

#[derive(Template)]
#[template(
    source = r#"
<section id="document-request-list" aria-live="polite">
  <h2>Your requests</h2>
  {% if requests.is_empty() %}
    <p>No document requests yet.</p>
  {% else %}
    <ul>
    {% for request in requests %}
      <li>
        <strong>{{ request.document_type_label }}</strong>
        — {{ request.status }}
        <time datetime="{{ request.requested_at }}">{{ request.requested_at }}</time>
      </li>
    {% endfor %}
    </ul>
  {% endif %}
</section>
"#,
    ext = "html"
)]
struct StudentRequestListTemplate<'a> {
    requests: &'a [StudentRequestView],
}

async fn render_student_request_list(actor: &Actor, pool: &PgPool) -> Result<String, AppError> {
    let student_id = actor.require_student_self()?;

    let rows = sqlx::query_as::<_, StudentRequestRow>(
        r#"
        SELECT document_type, status, requested_at
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

    let requests: Vec<_> = rows
        .into_iter()
        .map(|row| StudentRequestView {
            document_type_label: document_type_label(&row.document_type),
            status: row.status,
            requested_at: row.requested_at.to_rfc3339(),
        })
        .collect();

    Ok(StudentRequestListTemplate {
        requests: &requests,
    }
    .render()?)
}

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
    document_type_label: &'static str,
    student_number: String,
    purpose: String,
    requested_at: String,
}

#[derive(Template)]
#[template(
    source = r#"
<section id="document-queue">
  <h1>Pending document requests</h1>
  {% if requests.is_empty() %}
    <p>No pending requests.</p>
  {% endif %}
  {% for request in requests %}
  <article>
    <h2>{{ request.document_type_label }}</h2>
    <p>Student: {{ request.student_number }}</p>
    <p>Requested: {{ request.requested_at }}</p>
    <p>{{ request.purpose }}</p>
    <form method="post"
          action="/ui/admin/document-requests/{{ request.id }}/approve"
          x-target="document-queue">
      <input type="hidden" name="csrf_token" value="{{ csrf_token }}">
      <label>Approval note <textarea name="note"></textarea></label>
      <button type="submit">Approve</button>
    </form>
    <form method="post"
          action="/ui/admin/document-requests/{{ request.id }}/reject"
          x-target="document-queue"
          x-target.422="document-queue">
      <input type="hidden" name="csrf_token" value="{{ csrf_token }}">
      <label>Rejection reason <textarea name="note" required></textarea></label>
      <button type="submit">Reject</button>
    </form>
  </article>
  {% endfor %}
</section>
"#,
    ext = "html"
)]
struct AdminQueueTemplate<'a> {
    requests: &'a [PendingRequestView],
    csrf_token: &'a str,
}

async fn render_admin_queue(
    actor: &Actor,
    pool: &PgPool,
    csrf_token: &str,
) -> Result<String, AppError> {
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

    let requests: Vec<_> = rows
        .into_iter()
        .map(|row| PendingRequestView {
            id: row.id,
            document_type_label: document_type_label(&row.document_type),
            student_number: row.student_number,
            purpose: row.purpose.unwrap_or_default(),
            requested_at: row.requested_at.to_rfc3339(),
        })
        .collect();

    Ok(AdminQueueTemplate {
        requests: &requests,
        csrf_token,
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

pub fn routes(cfg: &mut web::ServiceConfig) {
    cfg.service(request_fragment)
        .service(approve_fragment)
        .service(reject_fragment)
        .service(download);
}
