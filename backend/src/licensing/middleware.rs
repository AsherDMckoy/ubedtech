//! License-gate middleware: requests to a locked institution answer 402.
//!
//! Runs before session resolution — the check is a lock-free snapshot read,
//! so a locked deployment refuses work before spending a database roundtrip
//! on the session. The exemption list is the recovery surface: health
//! probes, license status/import, the locked page, the session routes (a
//! platform licensing admin must be able to sign in to unlock), and the
//! platform operator UI, whose handlers enforce their own role checks.

use actix_web::body::{BoxBody, MessageBody};
use actix_web::dev::{ServiceRequest, ServiceResponse};
use actix_web::middleware::Next;
use actix_web::{ResponseError, web};

use crate::licensing::LicenseGate;
use crate::shared::error::AppError;

pub(crate) fn is_license_exempt(path: &str) -> bool {
    const EXACT: &[&str] = &[
        "/health/live",
        "/health/ready",
        "/license/status",
        "/license/import",
        "/institution-locked",
        "/api/v1/session/login",
        "/api/v1/session/logout",
        "/ui/login",
        "/ui/signout",
    ];
    // Static assets carry no institution data; the login and locked pages
    // need their stylesheet while locked.
    EXACT.contains(&path) || path.starts_with("/ui/platform/") || path.starts_with("/assets/")
}

pub async fn license_middleware(
    req: ServiceRequest,
    next: Next<impl MessageBody + 'static>,
) -> Result<ServiceResponse<BoxBody>, actix_web::Error> {
    if is_license_exempt(req.path()) {
        return Ok(next.call(req).await?.map_into_boxed_body());
    }

    // Missing app data is a wiring bug; fail the request, not the process.
    let Some(gate) = req.app_data::<web::Data<LicenseGate>>() else {
        tracing::error!("LicenseGate is not registered as app data");
        let (req, _) = req.into_parts();
        return Ok(ServiceResponse::new(
            req,
            AppError::Internal.error_response(),
        ));
    };

    if let Err(locked) = gate.require_deployment_active() {
        let (req, _) = req.into_parts();
        return Ok(ServiceResponse::new(req, locked.error_response()));
    }

    Ok(next.call(req).await?.map_into_boxed_body())
}

#[cfg(test)]
mod tests {
    use super::is_license_exempt;

    #[test]
    fn recovery_surface_is_exempt() {
        for path in [
            "/health/live",
            "/health/ready",
            "/license/status",
            "/license/import",
            "/institution-locked",
            "/api/v1/session/login",
            "/api/v1/session/logout",
            "/ui/platform/institutions/123/license",
            "/assets/app-0123abcd.css",
        ] {
            assert!(is_license_exempt(path), "{path} must stay reachable");
        }
    }

    #[test]
    fn protected_surface_is_not_exempt() {
        for path in [
            "/api/v1/me/enrollments",
            "/api/v1/me/grades",
            "/ui/registration/add",
            "/ui/registration/drop",
            "/health/liveX",
            "/ui/platformX",
            "/",
        ] {
            assert!(!is_license_exempt(path), "{path} must be gated");
        }
    }
}
