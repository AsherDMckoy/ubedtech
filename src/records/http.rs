use crate::shared::{actor::Actor, error::AppError};
use actix_web::{HttpResponse, get, post, web};
use serde::Deserialize;
use uuid::Uuid;

use crate::records::grades::{CorrectGradeCommand, SaveGradeCommand};
use crate::records::{GradeService, ScheduleQuery, TranscriptSnapshotService};
use sqlx::PgPool;

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

/// The instructor's own assigned sections.
#[get("/api/v1/instructor/sections")]
pub async fn instructor_sections_json(
    actor: Actor,
    service: web::Data<GradeService>,
) -> Result<HttpResponse, AppError> {
    Ok(HttpResponse::Ok().json(service.instructor_sections(&actor).await?))
}

/// Roster with grade states: assigned instructor or records officer.
#[get("/api/v1/sections/{section_id}/roster")]
pub async fn roster_json(
    actor: Actor,
    service: web::Data<GradeService>,
    section_id: web::Path<Uuid>,
) -> Result<HttpResponse, AppError> {
    Ok(HttpResponse::Ok().json(service.roster(&actor, section_id.into_inner()).await?))
}

/// Records officer freezes the student's published record into an immutable
/// snapshot (new monotonic version).
#[post("/api/v1/students/{student_id}/transcript-snapshots")]
pub async fn generate_snapshot_json(
    actor: Actor,
    service: web::Data<TranscriptSnapshotService>,
    pool: web::Data<PgPool>,
    student_id: web::Path<Uuid>,
) -> Result<HttpResponse, AppError> {
    let snapshot_id = service
        .generate(
            &pool,
            &crate::audit::AuditWriter,
            &actor,
            student_id.into_inner(),
        )
        .await?;
    Ok(HttpResponse::Created().json(serde_json::json!({ "snapshot_id": snapshot_id })))
}

/// The student's own academic history and snapshot list.
#[get("/api/v1/me/history")]
pub async fn history_json(
    actor: Actor,
    grades: web::Data<GradeService>,
    snapshots: web::Data<TranscriptSnapshotService>,
    pool: web::Data<PgPool>,
) -> Result<HttpResponse, AppError> {
    let courses = grades.academic_history(&actor).await?;
    let snapshots = snapshots.own_snapshots(&pool, &actor).await?;
    Ok(HttpResponse::Ok().json(serde_json::json!({
        "courses": courses,
        "snapshots": snapshots,
    })))
}

pub fn routes(cfg: &mut web::ServiceConfig) {
    cfg.service(grades_json)
        .service(schedule_json)
        .service(save_draft_json)
        .service(correct_grade_json)
        .service(publish_section_json)
        .service(instructor_sections_json)
        .service(roster_json)
        .service(generate_snapshot_json)
        .service(history_json);
}
