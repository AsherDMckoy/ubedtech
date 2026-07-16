use crate::shared::{actor::Actor, error::AppError};
use actix_web::{HttpResponse, post, web};
use serde::Deserialize;
use uuid::Uuid;

use crate::enrollment::{EnrollmentService, GrantOverrideCommand, RegisterCommand};

#[derive(Deserialize)]
pub struct RegisterForm {
    section_id: Uuid,
    idempotency_key: Uuid,
    csrf_token: String,
}

#[post("/api/v1/me/enrollments")]
pub async fn register_json(
    actor: Actor,
    service: web::Data<EnrollmentService>,
    body: web::Json<RegisterCommand>,
) -> Result<HttpResponse, AppError> {
    let receipt = service
        .register_self(&actor, body.into_inner())
        .await
        .map_err(AppError::from)?;
    Ok(HttpResponse::Created().json(receipt))
}

/// Registrar grants a registration override — the explicit, auditable record
/// (who, rule, why, expiry) that later gets stamped with the enrollment that
/// consumed it. Never a hidden boolean.
#[post("/api/v1/students/{student_id}/overrides")]
pub async fn grant_override(
    actor: Actor,
    service: web::Data<EnrollmentService>,
    student_id: web::Path<Uuid>,
    body: web::Json<GrantOverrideCommand>,
) -> Result<HttpResponse, AppError> {
    let override_id = service
        .grant_override(&actor, student_id.into_inner(), body.into_inner())
        .await?;
    Ok(HttpResponse::Created().json(serde_json::json!({ "override_id": override_id })))
}

#[post("/ui/registration/add")]
pub async fn register_fragment(
    actor: Actor,
    service: web::Data<EnrollmentService>,
    form: web::Form<RegisterForm>,
) -> Result<HttpResponse, AppError> {
    // CSRF middleware should validate the token before this handler. Keeping the
    // field in the form preserves non-JavaScript progressive enhancement.
    let _ = &form.csrf_token;

    service
        .register_self(
            &actor,
            RegisterCommand {
                section_id: form.section_id,
                idempotency_key: form.idempotency_key,
            },
        )
        .await
        .map_err(AppError::from)?;

    // In a real project render Askama templates. The response must contain every
    // element named by x-target.
    Ok(HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(render_registration_panel(&actor).await?))
}

#[derive(Deserialize)]
pub struct DropForm {
    enrollment_id: Uuid,
    csrf_token: String,
}

#[post("/ui/registration/drop")]
pub async fn drop_fragment(
    actor: Actor,
    service: web::Data<EnrollmentService>,
    form: web::Form<DropForm>,
) -> Result<HttpResponse, AppError> {
    let _ = &form.csrf_token;
    service
        .drop_self(&actor, form.enrollment_id)
        .await
        .map_err(AppError::from)?;

    Ok(HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(render_registration_panel(&actor).await?))
}

async fn render_registration_panel(_actor: &Actor) -> Result<String, AppError> {
    // Replace with an Askama template fed by one registration-page query.
    Ok(r#"
        <section id="registration-panel">
            <p role="status">Registration updated.</p>
        </section>
        <div id="notifications" x-sync></div>
    "#
    .to_owned())
}
pub fn routes(cfg: &mut web::ServiceConfig) {
    cfg.service(register_json)
        .service(grant_override)
        .service(register_fragment)
        .service(drop_fragment);
}
