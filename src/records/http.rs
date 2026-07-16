use crate::academics::AcademicsService;
use crate::academics::service::TermSummary;
use crate::identity_access::sessions::CurrentSession;
use crate::shared::actor::Role;
use crate::shared::{actor::Actor, error::AppError};
use actix_web::http::StatusCode;
use actix_web::{HttpResponse, get, post, web};
use askama::Template;
use serde::Deserialize;
use uuid::Uuid;

use crate::records::grades::{
    CorrectGradeCommand, HistoryRow, InstructorSection, RosterView, SaveGradeCommand,
    StudentGradeRow,
};
use crate::records::transcript::SnapshotSummary;
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

// ---------------------------------------------------------------------------
// Server-rendered pages (plain forms, no JavaScript required).
// ---------------------------------------------------------------------------

#[derive(Template)]
#[template(path = "pages/instructor_sections.html")]
struct InstructorSectionsPage {
    sections: Vec<InstructorSection>,
}

#[get("/ui/instructor")]
pub async fn instructor_sections_page(
    actor: Actor,
    service: web::Data<GradeService>,
) -> Result<HttpResponse, AppError> {
    let page = InstructorSectionsPage {
        sections: service.instructor_sections(&actor).await?,
    };
    Ok(html(StatusCode::OK, page.render()?))
}

#[derive(Template)]
#[template(path = "pages/roster.html")]
struct RosterPage<'a> {
    csrf_token: &'a str,
    view: RosterView,
    can_publish: bool,
    notice: Option<&'a str>,
    error: Option<&'a str>,
}

#[derive(Deserialize)]
pub struct RosterNoticeQuery {
    notice: Option<String>,
}

#[get("/ui/instructor/sections/{section_id}")]
pub async fn roster_page(
    actor: Actor,
    current: CurrentSession,
    service: web::Data<GradeService>,
    section_id: web::Path<Uuid>,
    query: web::Query<RosterNoticeQuery>,
) -> Result<HttpResponse, AppError> {
    let notice = match query.notice.as_deref() {
        Some("saved") => Some("Draft grade saved."),
        Some("published") => Some("Draft grades published."),
        _ => None,
    };
    let body = render_roster(
        &actor,
        &current,
        &service,
        section_id.into_inner(),
        notice,
        None,
    )
    .await?;
    Ok(html(StatusCode::OK, body))
}

#[derive(Deserialize)]
pub struct GradeEntryForm {
    section_id: Uuid,
    enrollment_id: Uuid,
    grade_code: String,
    /// Free text so an empty field is representable; parsed below.
    grade_points: Option<String>,
    expected_version: i64,
    csrf_token: String,
}

/// Draft entry from the roster form. Success redirects back (PRG); a denial
/// re-renders the roster with the reason inline.
#[post("/ui/instructor/grades")]
pub async fn save_grade_form(
    actor: Actor,
    current: CurrentSession,
    service: web::Data<GradeService>,
    form: web::Form<GradeEntryForm>,
) -> Result<HttpResponse, AppError> {
    let _ = &form.csrf_token; // validated by the CSRF middleware

    let points_text = form.grade_points.as_deref().unwrap_or("").trim();
    let grade_points = if points_text.is_empty() {
        Ok(None)
    } else {
        points_text
            .parse::<f64>()
            .map(Some)
            .map_err(|_| AppError::Validation("grade points must be a number".into()))
    };

    let outcome = match grade_points {
        Ok(grade_points) => service
            .save_draft(
                &actor,
                SaveGradeCommand {
                    enrollment_id: form.enrollment_id,
                    grade_code: form.grade_code.clone(),
                    grade_points,
                    numeric_value: None,
                    expected_version: form.expected_version,
                },
            )
            .await
            .map(|_| ()),
        Err(error) => Err(error),
    };

    match outcome {
        Ok(()) => Ok(see_other(&format!(
            "/ui/instructor/sections/{}?notice=saved",
            form.section_id
        ))),
        // Business denials render inline with their honest status; anything
        // else keeps its error shape.
        Err(error @ (AppError::Conflict(_) | AppError::Validation(_))) => {
            let status = match &error {
                AppError::Validation(_) => StatusCode::UNPROCESSABLE_ENTITY,
                _ => StatusCode::CONFLICT,
            };
            let message = error.to_string();
            let body = render_roster(
                &actor,
                &current,
                &service,
                form.section_id,
                None,
                Some(&message),
            )
            .await?;
            Ok(html(status, body))
        }
        Err(other) => Err(other),
    }
}

#[derive(Deserialize)]
pub struct PublishForm {
    csrf_token: String,
}

/// Publish every draft grade of the section (records officer).
#[post("/ui/instructor/sections/{section_id}/publish")]
pub async fn publish_form(
    actor: Actor,
    service: web::Data<GradeService>,
    section_id: web::Path<Uuid>,
    form: web::Form<PublishForm>,
) -> Result<HttpResponse, AppError> {
    let _ = &form.csrf_token;
    let section_id = section_id.into_inner();
    service.publish_section(&actor, section_id).await?;
    Ok(see_other(&format!(
        "/ui/instructor/sections/{section_id}?notice=published"
    )))
}

#[derive(Template)]
#[template(path = "pages/grades.html")]
struct StudentGradesPage {
    term: Option<TermSummary>,
    grades: Vec<StudentGradeRow>,
}

/// The student's published grades for the current term. Drafts are excluded
/// by the underlying query, not by this page.
#[get("/ui/grades")]
pub async fn student_grades_page(
    actor: Actor,
    grades: web::Data<GradeService>,
    academics: web::Data<AcademicsService>,
) -> Result<HttpResponse, AppError> {
    let term = academics.current_term(&actor).await?;
    let rows = match &term {
        Some(term) => grades.student_grades(&actor, term.id).await?,
        None => Vec::new(),
    };
    let page = StudentGradesPage { term, grades: rows };
    Ok(html(StatusCode::OK, page.render()?))
}

#[derive(Template)]
#[template(path = "pages/history.html")]
struct HistoryPage {
    courses: Vec<HistoryRow>,
    snapshots: Vec<SnapshotSummary>,
}

#[get("/ui/history")]
pub async fn history_page(
    actor: Actor,
    grades: web::Data<GradeService>,
    snapshots: web::Data<TranscriptSnapshotService>,
    pool: web::Data<PgPool>,
) -> Result<HttpResponse, AppError> {
    let page = HistoryPage {
        courses: grades.academic_history(&actor).await?,
        snapshots: snapshots.own_snapshots(&pool, &actor).await?,
    };
    Ok(html(StatusCode::OK, page.render()?))
}

async fn render_roster(
    actor: &Actor,
    current: &CurrentSession,
    service: &GradeService,
    section_id: Uuid,
    notice: Option<&str>,
    error: Option<&str>,
) -> Result<String, AppError> {
    let view = service.roster(actor, section_id).await?;
    Ok(RosterPage {
        csrf_token: &current.csrf_token,
        view,
        can_publish: actor.has_role(Role::RecordsOfficer),
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
    cfg.service(grades_json)
        .service(schedule_json)
        .service(save_draft_json)
        .service(correct_grade_json)
        .service(publish_section_json)
        .service(instructor_sections_json)
        .service(roster_json)
        .service(generate_snapshot_json)
        .service(history_json)
        .service(instructor_sections_page)
        .service(roster_page)
        .service(save_grade_form)
        .service(publish_form)
        .service(student_grades_page)
        .service(history_page);
}
