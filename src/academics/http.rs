use actix_web::{HttpResponse, get, post, put, web};
use serde::Deserialize;
use uuid::Uuid;

use crate::academics::AcademicsService;
use crate::academics::service::{
    AddMeetingCommand, AddPrerequisiteCommand, CreateCourseCommand, CreateSectionCommand,
    CreateTermCommand,
};
use crate::shared::{actor::Actor, error::AppError};

#[post("/api/v1/terms")]
async fn create_term(
    actor: Actor,
    service: web::Data<AcademicsService>,
    body: web::Json<CreateTermCommand>,
) -> Result<HttpResponse, AppError> {
    let id = service.create_term(&actor, body.into_inner()).await?;
    Ok(HttpResponse::Created().json(serde_json::json!({ "term_id": id })))
}

#[get("/api/v1/terms/current")]
async fn current_term(
    actor: Actor,
    service: web::Data<AcademicsService>,
) -> Result<HttpResponse, AppError> {
    match service.current_term(&actor).await? {
        Some(term) => Ok(HttpResponse::Ok().json(term)),
        None => Err(AppError::NotFound),
    }
}

#[post("/api/v1/courses")]
async fn create_course(
    actor: Actor,
    service: web::Data<AcademicsService>,
    body: web::Json<CreateCourseCommand>,
) -> Result<HttpResponse, AppError> {
    let id = service.create_course(&actor, body.into_inner()).await?;
    Ok(HttpResponse::Created().json(serde_json::json!({ "course_id": id })))
}

#[post("/api/v1/courses/{course_id}/prerequisites")]
async fn add_prerequisite(
    actor: Actor,
    service: web::Data<AcademicsService>,
    course_id: web::Path<Uuid>,
    body: web::Json<AddPrerequisiteCommand>,
) -> Result<HttpResponse, AppError> {
    service
        .add_prerequisite(&actor, course_id.into_inner(), body.into_inner())
        .await?;
    Ok(HttpResponse::NoContent().finish())
}

#[post("/api/v1/sections")]
async fn create_section(
    actor: Actor,
    service: web::Data<AcademicsService>,
    body: web::Json<CreateSectionCommand>,
) -> Result<HttpResponse, AppError> {
    let id = service.create_section(&actor, body.into_inner()).await?;
    Ok(HttpResponse::Created().json(serde_json::json!({ "section_id": id })))
}

#[derive(Deserialize)]
struct CapacityBody {
    capacity: i32,
}

#[put("/api/v1/sections/{section_id}/capacity")]
async fn set_capacity(
    actor: Actor,
    service: web::Data<AcademicsService>,
    section_id: web::Path<Uuid>,
    body: web::Json<CapacityBody>,
) -> Result<HttpResponse, AppError> {
    service
        .set_section_capacity(&actor, section_id.into_inner(), body.capacity)
        .await?;
    Ok(HttpResponse::NoContent().finish())
}

#[post("/api/v1/sections/{section_id}/meetings")]
async fn add_meeting(
    actor: Actor,
    service: web::Data<AcademicsService>,
    section_id: web::Path<Uuid>,
    body: web::Json<AddMeetingCommand>,
) -> Result<HttpResponse, AppError> {
    let id = service
        .add_meeting(&actor, section_id.into_inner(), body.into_inner())
        .await?;
    Ok(HttpResponse::Created().json(serde_json::json!({ "meeting_id": id })))
}

#[derive(Deserialize)]
struct CatalogQuery {
    term_id: Uuid,
    q: Option<String>,
    #[serde(default)]
    page: u32,
}

#[get("/api/v1/catalog")]
async fn catalog(
    actor: Actor,
    service: web::Data<AcademicsService>,
    query: web::Query<CatalogQuery>,
) -> Result<HttpResponse, AppError> {
    let sections = service
        .search_catalog(&actor, query.term_id, query.q.as_deref(), query.page)
        .await?;
    Ok(HttpResponse::Ok().json(sections))
}

pub fn routes(cfg: &mut web::ServiceConfig) {
    cfg.service(create_term)
        .service(current_term)
        .service(create_course)
        .service(add_prerequisite)
        .service(create_section)
        .service(set_capacity)
        .service(add_meeting)
        .service(catalog);
}
