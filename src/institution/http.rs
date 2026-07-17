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
        .service(delete_event_form);
}
