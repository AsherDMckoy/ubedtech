//! Institution-admin HTTP adapters: the events/holidays calendar page.
//! Plain forms (PRG on success, inline errors) like every other page.

use crate::identity_access::sessions::CurrentSession;
use crate::shared::{actor::Actor, error::AppError};
use actix_web::http::StatusCode;
use actix_web::{HttpResponse, get, post, web};
use askama::Template;
use chrono::NaiveDate;
use serde::Deserialize;
use uuid::Uuid;

use crate::institution::InstitutionService;

struct EventView {
    id: Uuid,
    title: String,
    kind_label: &'static str,
    starts_on: String,
    ends_on: String,
}

#[derive(Template)]
#[template(path = "pages/calendar_admin.html")]
struct CalendarPage<'a> {
    csrf_token: &'a str,
    events: Vec<EventView>,
    notice: Option<&'a str>,
    error: Option<&'a str>,
}

#[derive(Deserialize)]
pub struct NoticeQuery {
    notice: Option<String>,
}

#[get("/ui/admin/calendar")]
pub async fn calendar_page(
    actor: Actor,
    current: CurrentSession,
    service: web::Data<InstitutionService>,
    query: web::Query<NoticeQuery>,
) -> Result<HttpResponse, AppError> {
    let notice = match query.notice.as_deref() {
        Some("created") => Some("Event added."),
        Some("deleted") => Some("Event removed."),
        _ => None,
    };
    let body = render_calendar(&actor, &current, &service, notice, None).await?;
    Ok(html(StatusCode::OK, body))
}

#[derive(Deserialize)]
pub struct CreateEventForm {
    title: String,
    event_type: String,
    starts_on: NaiveDate,
    ends_on: NaiveDate,
    csrf_token: String,
}

#[post("/ui/admin/calendar")]
pub async fn create_event_form(
    actor: Actor,
    current: CurrentSession,
    service: web::Data<InstitutionService>,
    form: web::Form<CreateEventForm>,
) -> Result<HttpResponse, AppError> {
    let _ = &form.csrf_token; // validated by the CSRF middleware

    let outcome = service
        .create_event(
            &actor,
            crate::institution::service::EventCommand {
                title: form.title.clone(),
                event_type: form.event_type.clone(),
                starts_on: form.starts_on,
                ends_on: form.ends_on,
            },
        )
        .await;

    match outcome {
        Ok(_) => Ok(see_other("/ui/admin/calendar?notice=created")),
        Err(AppError::Validation(message)) => {
            let body = render_calendar(&actor, &current, &service, None, Some(&message)).await?;
            Ok(html(StatusCode::UNPROCESSABLE_ENTITY, body))
        }
        Err(AppError::Conflict(message)) => {
            let body = render_calendar(&actor, &current, &service, None, Some(&message)).await?;
            Ok(html(StatusCode::CONFLICT, body))
        }
        Err(other) => Err(other),
    }
}

#[derive(Deserialize)]
pub struct DeleteEventForm {
    csrf_token: String,
}

#[post("/ui/admin/calendar/{event_id}/delete")]
pub async fn delete_event_form(
    actor: Actor,
    current: CurrentSession,
    service: web::Data<InstitutionService>,
    event_id: web::Path<Uuid>,
    form: web::Form<DeleteEventForm>,
) -> Result<HttpResponse, AppError> {
    let _ = &form.csrf_token;

    match service.delete_event(&actor, event_id.into_inner()).await {
        Ok(()) => Ok(see_other("/ui/admin/calendar?notice=deleted")),
        Err(AppError::NotFound) => {
            let body = render_calendar(
                &actor,
                &current,
                &service,
                None,
                Some("that event no longer exists"),
            )
            .await?;
            Ok(html(StatusCode::NOT_FOUND, body))
        }
        Err(other) => Err(other),
    }
}

async fn render_calendar(
    actor: &Actor,
    current: &CurrentSession,
    service: &InstitutionService,
    notice: Option<&str>,
    error: Option<&str>,
) -> Result<String, AppError> {
    let events = service
        .list_events(actor)
        .await?
        .into_iter()
        .map(|event| EventView {
            id: event.id,
            title: event.title,
            kind_label: match event.event_type.as_str() {
                "holiday" => "Holiday",
                _ => "Event",
            },
            starts_on: event.starts_on.to_string(),
            ends_on: event.ends_on.to_string(),
        })
        .collect();

    Ok(CalendarPage {
        csrf_token: &current.csrf_token,
        events,
        notice,
        error,
    }
    .render()?)
}

/// Renders the calendar page with representative data and no database, for
/// the frontend axe harness.
pub fn sample_calendar_admin_html() -> Result<String, askama::Error> {
    CalendarPage {
        csrf_token: "sample",
        events: vec![
            EventView {
                id: Uuid::nil(),
                title: "Independence Day".into(),
                kind_label: "Holiday",
                starts_on: "2026-09-21".into(),
                ends_on: "2026-09-21".into(),
            },
            EventView {
                id: Uuid::nil(),
                title: "Open day".into(),
                kind_label: "Event",
                starts_on: "2026-10-03".into(),
                ends_on: "2026-10-04".into(),
            },
        ],
        notice: Some("Event added."),
        error: None,
    }
    .render()
}

// ---------------------------------------------------------------------------
// Settings page (name, timezone, document types) — plain forms over the
// same service functions as the JSON API below.
// ---------------------------------------------------------------------------

/// One document type row with its display label.
pub struct DocumentTypeView {
    pub document_type: String,
    pub label: &'static str,
    pub enabled: bool,
}

fn document_type_label(document_type: &str) -> &'static str {
    match document_type {
        "official_transcript" => "Official transcript",
        "enrollment_letter" => "Enrollment letter",
        _ => "Signed document",
    }
}

#[derive(Template)]
#[template(path = "pages/institution_settings.html")]
struct SettingsPage<'a> {
    csrf_token: &'a str,
    name: String,
    timezone: String,
    document_types: Vec<DocumentTypeView>,
    notice: Option<&'a str>,
    error: Option<&'a str>,
}

async fn render_settings(
    actor: &Actor,
    current: &CurrentSession,
    service: &InstitutionService,
    notice: Option<&str>,
    error: Option<&str>,
) -> Result<String, AppError> {
    let settings = service.settings(actor).await?;
    let document_types = service
        .document_type_settings(actor)
        .await?
        .into_iter()
        .map(|setting| DocumentTypeView {
            label: document_type_label(&setting.document_type),
            document_type: setting.document_type,
            enabled: setting.enabled,
        })
        .collect();
    Ok(SettingsPage {
        csrf_token: &current.csrf_token,
        name: settings.name,
        timezone: settings.timezone,
        document_types,
        notice,
        error,
    }
    .render()?)
}

#[get("/ui/admin/settings")]
pub async fn settings_page(
    actor: Actor,
    current: CurrentSession,
    service: web::Data<InstitutionService>,
    query: web::Query<NoticeQuery>,
) -> Result<HttpResponse, AppError> {
    let notice = match query.notice.as_deref() {
        Some("saved") => Some("Settings saved."),
        Some("document-type") => Some("Document type updated."),
        _ => None,
    };
    let body = render_settings(&actor, &current, &service, notice, None).await?;
    Ok(html(StatusCode::OK, body))
}

#[derive(Deserialize)]
pub struct SettingsForm {
    name: String,
    timezone: String,
    csrf_token: String,
}

#[post("/ui/admin/settings")]
pub async fn settings_form(
    actor: Actor,
    current: CurrentSession,
    service: web::Data<InstitutionService>,
    form: web::Form<SettingsForm>,
) -> Result<HttpResponse, AppError> {
    let _ = &form.csrf_token;
    match service
        .update_settings(&actor, &form.name, &form.timezone)
        .await
    {
        Ok(()) => Ok(see_other("/ui/admin/settings?notice=saved")),
        Err(AppError::Validation(message)) => {
            let body = render_settings(&actor, &current, &service, None, Some(&message)).await?;
            Ok(html(StatusCode::UNPROCESSABLE_ENTITY, body))
        }
        Err(other) => Err(other),
    }
}

#[derive(Deserialize)]
pub struct DocumentTypeForm {
    document_type: String,
    enable: bool,
    csrf_token: String,
}

#[post("/ui/admin/settings/document-types")]
pub async fn document_type_form(
    actor: Actor,
    current: CurrentSession,
    service: web::Data<InstitutionService>,
    form: web::Form<DocumentTypeForm>,
) -> Result<HttpResponse, AppError> {
    let _ = &form.csrf_token;
    match service
        .set_document_type_enabled(&actor, &form.document_type, form.enable)
        .await
    {
        Ok(()) => Ok(see_other("/ui/admin/settings?notice=document-type")),
        Err(AppError::Validation(message)) => {
            let body = render_settings(&actor, &current, &service, None, Some(&message)).await?;
            Ok(html(StatusCode::UNPROCESSABLE_ENTITY, body))
        }
        Err(other) => Err(other),
    }
}

/// Renders the settings page with representative data and no database, for
/// the frontend axe harness.
pub fn sample_settings_html() -> Result<String, askama::Error> {
    SettingsPage {
        csrf_token: "sample",
        name: "University of Belize".into(),
        timezone: "America/Belize".into(),
        document_types: vec![
            DocumentTypeView {
                document_type: "official_transcript".into(),
                label: "Official transcript",
                enabled: true,
            },
            DocumentTypeView {
                document_type: "enrollment_letter".into(),
                label: "Enrollment letter",
                enabled: true,
            },
            DocumentTypeView {
                document_type: "signed_document".into(),
                label: "Signed document",
                enabled: false,
            },
        ],
        notice: None,
        error: Some("unknown timezone"),
    }
    .render()
}

// ---------------------------------------------------------------------------
// Settings + document-type configuration (JSON, institution admin only).
// ---------------------------------------------------------------------------

#[actix_web::get("/api/v1/institution/settings")]
pub async fn get_settings(
    actor: Actor,
    service: web::Data<InstitutionService>,
) -> Result<HttpResponse, AppError> {
    let settings = service.settings(&actor).await?;
    let document_types = service.document_type_settings(&actor).await?;
    Ok(HttpResponse::Ok().json(serde_json::json!({
        "name": settings.name,
        "timezone": settings.timezone,
        "document_types": document_types,
    })))
}

#[derive(Deserialize)]
pub struct UpdateSettingsBody {
    name: String,
    timezone: String,
}

#[actix_web::put("/api/v1/institution/settings")]
pub async fn put_settings(
    actor: Actor,
    service: web::Data<InstitutionService>,
    body: web::Json<UpdateSettingsBody>,
) -> Result<HttpResponse, AppError> {
    service
        .update_settings(&actor, &body.name, &body.timezone)
        .await?;
    Ok(HttpResponse::NoContent().finish())
}

#[derive(Deserialize)]
pub struct DocumentTypeBody {
    enabled: bool,
}

#[actix_web::put("/api/v1/institution/document-types/{document_type}")]
pub async fn put_document_type(
    actor: Actor,
    service: web::Data<InstitutionService>,
    document_type: web::Path<String>,
    body: web::Json<DocumentTypeBody>,
) -> Result<HttpResponse, AppError> {
    service
        .set_document_type_enabled(&actor, &document_type, body.enabled)
        .await?;
    Ok(HttpResponse::NoContent().finish())
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
    cfg.service(calendar_page)
        .service(create_event_form)
        .service(delete_event_form)
        .service(settings_page)
        .service(settings_form)
        .service(document_type_form)
        .service(get_settings)
        .service(put_settings)
        .service(put_document_type);
}
