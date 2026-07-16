use crate::shared::{actor::Actor, error::AppError};
use actix_web::{HttpResponse, get, post, web};
use serde::Deserialize;
use uuid::Uuid;

use crate::records::grades::{CorrectGradeCommand, SaveGradeCommand};
use crate::records::{GradeService, ScheduleQuery};

#[derive(Deserialize)]
pub struct TermQuery {
    term_id: Uuid,
}

#[get("/api/v1/me/grades")]
pub async fn grades_json(
    actor: Actor,
    service: web::Data<GradeService>,
    query: web::Query<TermQuery>,
) -> Result<HttpResponse, AppError> {
    Ok(HttpResponse::Ok().json(service.student_grades(&actor, query.term_id).await?))
}

#[get("/api/v1/me/schedule")]
pub async fn schedule_json(
    actor: Actor,
    query_service: web::Data<ScheduleQuery>,
    query: web::Query<TermQuery>,
) -> Result<HttpResponse, AppError> {
    Ok(HttpResponse::Ok().json(query_service.for_student(&actor, query.term_id).await?))
}

/// Draft entry: assigned instructor (inside the entry window) or records
/// officer. Returns the new optimistic version.
#[post("/api/v1/grades/draft")]
pub async fn save_draft_json(
    actor: Actor,
    service: web::Data<GradeService>,
    body: web::Json<SaveGradeCommand>,
) -> Result<HttpResponse, AppError> {
    let version = service.save_draft(&actor, body.into_inner()).await?;
    Ok(HttpResponse::Ok().json(serde_json::json!({ "version": version })))
}

/// Records-office correction of a published grade (reason required).
#[post("/api/v1/grades/correct")]
pub async fn correct_grade_json(
    actor: Actor,
    service: web::Data<GradeService>,
    body: web::Json<CorrectGradeCommand>,
) -> Result<HttpResponse, AppError> {
    let version = service.correct_grade(&actor, body.into_inner()).await?;
    Ok(HttpResponse::Ok().json(serde_json::json!({ "version": version })))
}

/// Publish every draft grade of a section (records officer).
#[post("/api/v1/sections/{section_id}/grades/publish")]
pub async fn publish_section_json(
    actor: Actor,
    service: web::Data<GradeService>,
    section_id: web::Path<Uuid>,
) -> Result<HttpResponse, AppError> {
    let published = service
        .publish_section(&actor, section_id.into_inner())
        .await?;
    Ok(HttpResponse::Ok().json(serde_json::json!({ "published": published })))
}

pub fn routes(cfg: &mut web::ServiceConfig) {
    cfg.service(grades_json)
        .service(schedule_json)
        .service(save_draft_json)
        .service(correct_grade_json)
        .service(publish_section_json);
}
