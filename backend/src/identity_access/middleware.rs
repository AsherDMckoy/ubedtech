//! Session-resolution middleware: the piece that turns a cookie into an
//! `Actor` in request extensions. Handlers that declare an `Actor` (or
//! `CurrentSession`) parameter get 401 automatically when nothing valid was
//! resolved — this middleware itself never rejects, so anonymous routes
//! (login, health) keep working without an allowlist here.

use actix_web::body::MessageBody;
use actix_web::dev::{ServiceRequest, ServiceResponse};
use actix_web::middleware::Next;
use actix_web::{HttpMessage, web};

use crate::identity_access::sessions::{CurrentSession, SessionService};

/// Session cookie name. The raw value is the opaque token; everything else
/// about the session lives server-side.
pub const SESSION_COOKIE: &str = "ub_session";

pub async fn session_middleware(
    req: ServiceRequest,
    next: Next<impl MessageBody>,
) -> Result<ServiceResponse<impl MessageBody>, actix_web::Error> {
    if let Some(cookie) = req.cookie(SESSION_COOKIE) {
        // Missing app data is a wiring bug; fail the request, not the process.
        let Some(sessions) = req.app_data::<web::Data<SessionService>>() else {
            tracing::error!("SessionService is not registered as app data");
            return Err(crate::shared::error::AppError::Internal.into());
        };
        let sessions = sessions.clone();

        // A database failure here is a real 500, not an anonymous request:
        // silently downgrading an authenticated user would turn an outage
        // into a confusing wall of 401s and hide the actual fault.
        if let Some(resolved) = sessions.resolve(cookie.value()).await? {
            req.extensions_mut().insert(resolved.actor);
            req.extensions_mut().insert(CurrentSession {
                session_id: resolved.session_id,
                csrf_token_hash: resolved.csrf_token_hash,
                csrf_token: resolved.csrf_token,
            });
        }
    }

    next.call(req).await
}
