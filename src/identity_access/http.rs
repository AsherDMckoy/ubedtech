//! Session lifecycle HTTP adapters: login and logout.
//!
//! Login is deliberately reachable without a session and outside the license
//! gate (a platform licensing admin must be able to sign in to unlock a
//! locked institution). It is exempt from the CSRF-token requirement — it
//! carries no ambient authority to forge: authentication comes entirely from
//! the credentials in the body, and the session cookie is `SameSite=Lax`.

use actix_web::cookie::{Cookie, SameSite, time};
use actix_web::{HttpMessage, HttpRequest, HttpResponse, post, web};
use serde::{Deserialize, Serialize};

use crate::identity_access::middleware::SESSION_COOKIE;
use crate::identity_access::service::AuthService;
use crate::identity_access::sessions::{CurrentSession, SessionService};
use crate::licensing::LicenseGate;
use crate::shared::error::AppError;

/// Cookie attributes decided at startup: `Secure` in production, and
/// Max-Age mirroring the absolute session deadline.
#[derive(Clone, Copy)]
pub struct SessionCookiePolicy {
    pub secure: bool,
    pub max_age_secs: i64,
}

#[derive(Deserialize)]
pub struct LoginRequest {
    username: String,
    password: String,
}

#[derive(Serialize)]
struct LoginResponse {
    /// The session's CSRF token, shown exactly once; state-changing requests
    /// must present it (header or form field) from now on.
    csrf_token: String,
}

#[post("/api/v1/session/login")]
pub async fn login(
    req: HttpRequest,
    auth: web::Data<AuthService>,
    sessions: web::Data<SessionService>,
    gate: web::Data<LicenseGate>,
    policy: web::Data<SessionCookiePolicy>,
    body: web::Json<LoginRequest>,
) -> Result<HttpResponse, AppError> {
    // Throttling keys on the socket peer address: it cannot be spoofed,
    // unlike forwarded headers. Behind a reverse proxy this collapses to the
    // proxy's address — see SECURITY.md before deploying one.
    let client_ip = req
        .peer_addr()
        .map(|addr| addr.ip().to_string())
        .unwrap_or_else(|| "unknown".to_owned());

    // Logging in over an existing session rotates it away: the old token is
    // dead no matter what the browser still holds.
    let presented = req.extensions().get::<CurrentSession>().cloned();
    if let Some(old) = presented {
        sessions.revoke(old.session_id).await?;
    }

    // Single-tenant deployment: the institution is the one the license was
    // loaded for at startup.
    let institution_id = gate.snapshot().institution_id;

    let outcome = auth
        .login(institution_id, &body.username, &body.password, &client_ip)
        .await?;

    // Auth events are logged by ids only — never the username that was
    // typed (may be a mistyped password) and never any token material.
    tracing::info!(
        user_id = %outcome.user_id,
        session_id = %outcome.session.session_id,
        "login succeeded"
    );

    let cookie = Cookie::build(SESSION_COOKIE, outcome.session.token.expose().to_owned())
        .http_only(true)
        .secure(policy.secure)
        .same_site(SameSite::Lax)
        .path("/")
        .max_age(time::Duration::seconds(policy.max_age_secs))
        .finish();

    Ok(HttpResponse::Ok().cookie(cookie).json(LoginResponse {
        csrf_token: outcome.session.csrf_token,
    }))
}

#[post("/api/v1/session/logout")]
pub async fn logout(
    current: CurrentSession,
    sessions: web::Data<SessionService>,
) -> Result<HttpResponse, AppError> {
    sessions.revoke(current.session_id).await?;

    let mut removal = Cookie::new(SESSION_COOKIE, "");
    removal.set_path("/");
    removal.make_removal();

    Ok(HttpResponse::NoContent().cookie(removal).finish())
}

pub fn routes(cfg: &mut web::ServiceConfig) {
    cfg.service(login).service(logout);
}
