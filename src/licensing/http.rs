use crate::shared::{actor::Actor, error::AppError};
use actix_web::{HttpResponse, post, web};
use serde::Deserialize;
use uuid::Uuid;

use crate::licensing::{LicenseService, LicenseStatus};

#[derive(Deserialize)]
pub struct LicenseStatusForm {
    status: String,
    reason: String,
    // Consumed by the CSRF middleware; kept so the form deserializes.
    #[serde(rename = "csrf_token")]
    _csrf_token: String,
}

#[post("/ui/platform/institutions/{institution_id}/license")]
pub async fn change_license_fragment(
    actor: Actor,
    service: web::Data<LicenseService>,
    institution_id: web::Path<Uuid>,
    form: web::Form<LicenseStatusForm>,
) -> Result<HttpResponse, AppError> {
    let status = match form.status.as_str() {
        "active" => LicenseStatus::Active,
        "suspended" => LicenseStatus::Suspended,
        "expired" => LicenseStatus::Expired,
        _ => return Err(AppError::Validation("unknown license status".into())),
    };

    let snapshot = service
        .set_status(&actor, institution_id.into_inner(), status, &form.reason)
        .await?;

    Ok(HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(format!(
            r#"<section id="license-panel"><p role="status">License status updated to {:?}. Version {}.</p></section>"#,
            snapshot.status, snapshot.version
        )))
}

pub fn routes(cfg: &mut web::ServiceConfig) {
    cfg.service(change_license_fragment);
}
