use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use actix_web::body::MessageBody;
use actix_web::dev::ServiceResponse;
use actix_web::http::{Method, StatusCode, header};
use actix_web::middleware::{ErrorHandlerResponse, ErrorHandlers};
use actix_web::{HttpResponse, ResponseError, middleware::DefaultHeaders, web};
use sqlx::PgPool;

use crate::config::Environment;

// Handlers here declare an `Actor` parameter, which the session middleware
// populates from the session cookie; without a valid session they 401. The
// license middleware (slice 2.5) additionally locks these behind an active
// license.
pub fn protected_routes(cfg: &mut web::ServiceConfig) {
    cfg.configure(crate::academics::http::routes)
        .configure(crate::enrollment::http::routes)
        .configure(crate::records::http::routes)
        .configure(crate::documents::http::routes)
        .configure(crate::institution::http::routes);
}

// Reachable while the institution license is locked (see
// licensing::middleware::is_license_exempt): probes, license recovery, the
// session routes, and the platform license-management fragment — a platform
// licensing admin has to be able to sign in and unlock a locked deployment.
pub fn recovery_routes(cfg: &mut web::ServiceConfig) {
    // Stylesheet/script must load on the login and locked pages too.
    cfg.configure(crate::shared::assets::routes);
    cfg.route("/health/live", web::get().to(health_live));
    cfg.route("/health/ready", web::get().to(health_ready));
    cfg.route("/license/status", web::get().to(license_status));
    // POST /license/import lives in licensing::http (self-hosted recovery).
    cfg.route("/institution-locked", web::get().to(locked_page));
    cfg.configure(crate::identity_access::http::routes);
    cfg.configure(crate::licensing::http::routes);
}

/// Liveness: the process is running and the event loop answers. No
/// dependencies are consulted — a database outage must not make an
/// orchestrator restart a healthy process.
async fn health_live() -> HttpResponse {
    HttpResponse::Ok().body("live")
}

/// Readiness: safe to route traffic here. Reads a cached flag maintained by
/// the background prober — a probe never performs database work itself, so
/// probe frequency cannot add load.
async fn health_ready(readiness: web::Data<Readiness>) -> HttpResponse {
    if readiness.is_ready() {
        HttpResponse::Ok().body("ready")
    } else {
        HttpResponse::ServiceUnavailable().body("not ready")
    }
}

/// Current license state, readable while locked so an operator can see WHY
/// requests are answering 402. Validity window and version only — no
/// feature set, no internal identifiers.
async fn license_status(gate: web::Data<crate::licensing::LicenseGate>) -> HttpResponse {
    let snapshot = gate.snapshot();
    HttpResponse::Ok().json(serde_json::json!({
        "status": snapshot.status,
        "valid_from": snapshot.valid_from,
        "valid_until": snapshot.valid_until,
        "version": snapshot.version,
    }))
}

/// The access-suspended screen browsers land on while the license is
/// inactive (the license middleware redirects `/ui/` GETs here).
#[derive(askama::Template)]
#[template(path = "pages/locked.html")]
struct LockedPage;

pub(crate) fn locked_html() -> Result<String, askama::Error> {
    use askama::Template;
    LockedPage.render()
}

async fn locked_page() -> HttpResponse {
    match locked_html() {
        Ok(body) => HttpResponse::Ok()
            .content_type("text/html; charset=utf-8")
            .body(body),
        Err(error) => {
            tracing::error!(%error, "rendering the locked page failed");
            crate::shared::error::AppError::Internal.error_response()
        }
    }
}

/// Shared readiness flag: written by the prober task, read by every
/// `/health/ready` probe.
#[derive(Clone)]
pub struct Readiness {
    ready: Arc<AtomicBool>,
}

impl Readiness {
    pub fn new(initially_ready: bool) -> Self {
        Self {
            ready: Arc::new(AtomicBool::new(initially_ready)),
        }
    }

    pub fn set(&self, ready: bool) {
        self.ready.store(ready, Ordering::Release);
    }

    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }
}

/// Re-checks the database on a fixed interval and updates the shared flag.
/// The check is bounded by the same interval so a hung database flips the
/// flag instead of stacking probes.
pub fn spawn_readiness_prober(
    pool: PgPool,
    readiness: Readiness,
    interval_secs: u64,
) -> actix_web::rt::task::JoinHandle<()> {
    actix_web::rt::spawn(async move {
        let interval = Duration::from_secs(interval_secs);
        loop {
            tokio::time::sleep(interval).await;

            let check = tokio::time::timeout(
                interval,
                sqlx::query_scalar::<_, i32>("SELECT 1").fetch_one(&pool),
            )
            .await;

            let healthy = matches!(check, Ok(Ok(1)));
            if healthy != readiness.is_ready() {
                tracing::warn!(ready = healthy, "readiness state changed");
            }
            readiness.set(healthy);
        }
    })
}

/// A signed-out browser navigating to a protected UI page lands on the
/// sign-in page instead of a bare 401. Only GET under `/ui/` is rewritten
/// — API clients keep their JSON 401, and the sign-in POST keeps its
/// inline 401 re-render.
pub fn login_redirects<B: MessageBody + 'static>() -> ErrorHandlers<B> {
    ErrorHandlers::new().handler(StatusCode::UNAUTHORIZED, |response: ServiceResponse<B>| {
        let request = response.request();
        if request.method() == Method::GET
            && request.path().starts_with("/ui/")
            && request.path() != "/ui/login"
        {
            let redirect = HttpResponse::SeeOther()
                .insert_header((header::LOCATION, "/ui/login"))
                .finish()
                .map_into_right_body();
            let (request, _) = response.into_parts();
            return Ok(ErrorHandlerResponse::Response(ServiceResponse::new(
                request, redirect,
            )));
        }
        Ok(ErrorHandlerResponse::Response(
            response.map_into_left_body(),
        ))
    })
}

/// Default security headers for every response.
///
/// The CSP is compatible with the Alpine.js CSP build: `script-src 'self'`
/// only — no `unsafe-eval` and no `unsafe-inline` anywhere. HSTS is emitted
/// only in production because development runs plain HTTP.
pub fn security_headers(environment: Environment) -> DefaultHeaders {
    let headers = DefaultHeaders::new()
        // Default for every dynamic response: authenticated pages carry
        // grades and documents, and no shared cache may keep them.
        // DefaultHeaders does not overwrite, so the fingerprinted assets
        // keep their public/immutable lifetime (Phase 5 debt item 5.2).
        .add(("Cache-Control", "private, no-store"))
        .add(("X-Content-Type-Options", "nosniff"))
        .add(("X-Frame-Options", "DENY"))
        .add(("Referrer-Policy", "strict-origin-when-cross-origin"))
        .add((
            "Permissions-Policy",
            "camera=(), microphone=(), geolocation=()",
        ))
        .add((
            "Content-Security-Policy",
            "default-src 'self'; script-src 'self'; style-src 'self'; \
             img-src 'self' data:; font-src 'self'; connect-src 'self'; \
             frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
        ));

    if environment.is_production() {
        headers.add((
            "Strict-Transport-Security",
            "max-age=31536000; includeSubDomains",
        ))
    } else {
        headers
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::test as actix_test;
    use actix_web::{App, http::StatusCode};

    #[actix_web::test]
    async fn health_live_answers_without_any_state() {
        let app =
            actix_test::init_service(App::new().route("/health/live", web::get().to(health_live)))
                .await;

        let response = actix_test::call_service(
            &app,
            actix_test::TestRequest::get()
                .uri("/health/live")
                .to_request(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[actix_web::test]
    async fn health_ready_reflects_the_cached_flag() {
        let readiness = Readiness::new(true);
        let app = actix_test::init_service(
            App::new()
                .app_data(web::Data::new(readiness.clone()))
                .route("/health/ready", web::get().to(health_ready)),
        )
        .await;

        let response = actix_test::call_service(
            &app,
            actix_test::TestRequest::get()
                .uri("/health/ready")
                .to_request(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        readiness.set(false);
        let response = actix_test::call_service(
            &app,
            actix_test::TestRequest::get()
                .uri("/health/ready")
                .to_request(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    async fn header_map(environment: Environment) -> actix_web::http::header::HeaderMap {
        let app = actix_test::init_service(
            App::new()
                .wrap(security_headers(environment))
                .route("/probe", web::get().to(health_live)),
        )
        .await;

        let response = actix_test::call_service(
            &app,
            actix_test::TestRequest::get().uri("/probe").to_request(),
        )
        .await;
        response.headers().clone()
    }

    #[actix_web::test]
    async fn security_headers_are_present_and_csp_is_alpine_csp_compatible() {
        let headers = header_map(Environment::Development).await;

        assert_eq!(headers.get("X-Content-Type-Options").unwrap(), "nosniff");
        assert_eq!(headers.get("X-Frame-Options").unwrap(), "DENY");
        assert_eq!(
            headers.get("Cache-Control").unwrap(),
            "private, no-store",
            "dynamic responses must not be cacheable by shared caches"
        );
        assert_eq!(
            headers.get("Referrer-Policy").unwrap(),
            "strict-origin-when-cross-origin"
        );
        assert!(headers.contains_key("Permissions-Policy"));

        let csp = headers
            .get("Content-Security-Policy")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(csp.contains("script-src 'self'"));
        assert!(csp.contains("frame-ancestors 'none'"));
        assert!(
            !csp.contains("unsafe-eval") && !csp.contains("unsafe-inline"),
            "CSP must stay compatible with the Alpine CSP build: {csp}"
        );
    }

    #[actix_web::test]
    async fn hsts_is_production_only() {
        let dev = header_map(Environment::Development).await;
        assert!(
            !dev.contains_key("Strict-Transport-Security"),
            "HSTS must not be sent over plain-HTTP development"
        );

        let prod = header_map(Environment::Production).await;
        assert_eq!(
            prod.get("Strict-Transport-Security").unwrap(),
            "max-age=31536000; includeSubDomains"
        );
    }
}
