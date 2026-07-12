use actix_web::web;

// Phase 2 wraps these routes with session, CSRF, payload-limit, and license
// middleware (LicenseGate::require_active). Until then they are reachable but
// every handler 401s because nothing populates the Actor extension.
pub fn protected_routes(cfg: &mut web::ServiceConfig) {
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

async fn health() -> &'static str {
    "ok"
}
async fn license_status() -> &'static str {
    "status"
}
async fn import_license() -> &'static str {
    "import"
}
async fn locked_page() -> &'static str {
    "Institution access is inactive."
}
