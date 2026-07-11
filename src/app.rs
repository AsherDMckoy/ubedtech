use actix_web::web;
use crate::licensing::LicenseGate;

pub fn protected_routes(cfg: &mut web::ServiceConfig) {
//    cfg.service(
        //web::scope("/api/v1")
            // Wrap with authentication, CSRF for mutating methods, payload limits,
            // and a custom license middleware that calls LicenseGate::require_active.
    cfg.configure(crate::enrollment::http::routes)
        .configure(crate::records::http::routes)
        .configure(crate::documents::http::routes);
}

pub fn recovery_routes(cfg: &mut web::ServiceConfig) {
    cfg.route("/health", web::get().to(health));
    cfg.route("/license/status", web::get().to(license_status));
    cfg.route("/license/import", web::post().to(import_license));
    cfg.route("/institution-locked", web::get().to(locked_page));
}

async fn health() -> &'static str { "ok" }
async fn license_status() -> &'static str { "status" }
async fn import_license() -> &'static str { "import" }
async fn locked_page() -> &'static str { "Institution access is inactive." }

fn license_decision(
    gate: &LicenseGate,
    institution_id: uuid::Uuid,
) -> Result<(), crate::shared::error::AppError> {
    gate.require_active(institution_id)
}
