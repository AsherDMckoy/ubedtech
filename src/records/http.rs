use actix_web::{get, web, HttpResponse};
use crate::shared::{actor::Actor, error::AppError};
use serde::Deserialize;
use uuid::Uuid;

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
    Ok(HttpResponse::Ok().json(
        service.student_grades(&actor, query.term_id).await?,
    ))
}

#[get("/api/v1/me/schedule")]
pub async fn schedule_json(
    actor: Actor,
    query_service: web::Data<ScheduleQuery>,
    query: web::Query<TermQuery>,
) -> Result<HttpResponse, AppError> {
    Ok(HttpResponse::Ok().json(
        query_service.for_student(&actor, query.term_id).await?,
    ))
}

pub fn routes(cfg: &mut web::ServiceConfig) {
    cfg.service(grades_json).service(schedule_json);
}
