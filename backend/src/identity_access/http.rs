//! Session lifecycle HTTP adapters: login, logout, password change/reset,
//! and suspension.
//!
//! Login is deliberately reachable without a session and outside the license
//! gate (a platform licensing admin must be able to sign in to unlock a
//! locked institution). It is exempt from the CSRF-token requirement — it
//! carries no ambient authority to forge: authentication comes entirely from
//! the credentials in the body, and the session cookie is `SameSite=Lax`.

use actix_web::cookie::{Cookie, SameSite, time};
use actix_web::http::StatusCode;
use actix_web::{HttpMessage, HttpRequest, HttpResponse, delete, get, post, web};
use askama::Template;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::identity_access::middleware::SESSION_COOKIE;
use crate::identity_access::service::AuthService;
use crate::identity_access::sessions::{CurrentSession, NewSession, SessionService};
use crate::licensing::LicenseGate;
use crate::shared::actor::{Actor, Role};
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
    let session = authenticate(
        &req,
        &auth,
        &sessions,
        &gate,
        &body.username,
        &body.password,
    )
    .await?;
    Ok(session_response(&policy, session))
}

/// The shared login sequence for the JSON API and the HTML form: throttle by
/// socket peer address (unspoofable — see SECURITY.md before fronting with a
/// proxy), rotate away any presented session, authenticate against the
/// license snapshot's institution, log ids only.
async fn authenticate(
    req: &HttpRequest,
    auth: &AuthService,
    sessions: &SessionService,
    gate: &LicenseGate,
    username: &str,
    password: &str,
) -> Result<NewSession, AppError> {
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
        .login(institution_id, username, password, &client_ip)
        .await?;

    // Auth events are logged by ids only — never the username that was
    // typed (may be a mistyped password) and never any token material.
    tracing::info!(
        user_id = %outcome.user_id,
        session_id = %outcome.session.session_id,
        "login succeeded"
    );
    Ok(outcome.session)
}

#[derive(Template)]
#[template(path = "pages/login.html")]
struct LoginPage<'a> {
    error: Option<&'a str>,
}

/// HTML login for the no-JavaScript flow the student pages degrade to.
#[get("/ui/login")]
pub async fn login_page() -> Result<HttpResponse, AppError> {
    Ok(HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(LoginPage { error: None }.render()?))
}

#[post("/ui/login")]
pub async fn login_form(
    req: HttpRequest,
    auth: web::Data<AuthService>,
    sessions: web::Data<SessionService>,
    gate: web::Data<LicenseGate>,
    policy: web::Data<SessionCookiePolicy>,
    form: web::Form<LoginRequest>,
) -> Result<HttpResponse, AppError> {
    match authenticate(
        &req,
        &auth,
        &sessions,
        &gate,
        &form.username,
        &form.password,
    )
    .await
    {
        Ok(session) => Ok(HttpResponse::SeeOther()
            .cookie(session_cookie(&policy, &session))
            .insert_header(("Location", "/ui/registration"))
            .finish()),
        // The uniform 401 renders inline; everything else (throttling,
        // faults) keeps its JSON error shape and status.
        Err(AppError::Unauthenticated) => Ok(HttpResponse::build(StatusCode::UNAUTHORIZED)
            .content_type("text/html; charset=utf-8")
            .body(
                LoginPage {
                    error: Some("Invalid username or password."),
                }
                .render()?,
            )),
        Err(other) => Err(other),
    }
}

fn session_cookie(policy: &SessionCookiePolicy, session: &NewSession) -> Cookie<'static> {
    Cookie::build(SESSION_COOKIE, session.token.expose().to_owned())
        .http_only(true)
        .secure(policy.secure)
        .same_site(SameSite::Lax)
        .path("/")
        .max_age(time::Duration::seconds(policy.max_age_secs))
        .finish()
}

/// Builds the "here is your fresh session" response shared by login and
/// self-service password change: hardened cookie + one-time CSRF token.
fn session_response(policy: &SessionCookiePolicy, session: NewSession) -> HttpResponse {
    let cookie = session_cookie(policy, &session);
    HttpResponse::Ok().cookie(cookie).json(LoginResponse {
        csrf_token: session.csrf_token,
    })
}

#[derive(Template)]
#[template(path = "pages/signout.html")]
struct SignoutPage<'a> {
    csrf_token: &'a str,
}

/// Sign-out confirmation for the browser flow: a real CSRF-protected form,
/// so a cross-site GET can never end a session.
#[get("/ui/signout")]
pub async fn signout_page(current: CurrentSession) -> Result<HttpResponse, AppError> {
    Ok(HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(
            SignoutPage {
                csrf_token: &current.csrf_token,
            }
            .render()?,
        ))
}

#[derive(Deserialize)]
pub struct SignoutForm {
    csrf_token: String,
}

#[post("/ui/signout")]
pub async fn signout_form(
    current: CurrentSession,
    sessions: web::Data<SessionService>,
    form: web::Form<SignoutForm>,
) -> Result<HttpResponse, AppError> {
    let _ = &form.csrf_token; // validated by the CSRF middleware
    sessions.revoke(current.session_id).await?;

    let mut removal = Cookie::new(SESSION_COOKIE, "");
    removal.set_path("/");
    removal.make_removal();

    Ok(HttpResponse::SeeOther()
        .cookie(removal)
        .insert_header(("Location", "/ui/login"))
        .finish())
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

#[derive(Deserialize)]
pub struct ChangePasswordRequest {
    current_password: String,
    new_password: String,
}

/// Self-service password change. Every session dies server-side; this
/// client alone gets a replacement cookie and CSRF token, so the browser
/// stays signed in through the rotation.
#[post("/api/v1/me/password")]
pub async fn change_own_password(
    actor: Actor,
    auth: web::Data<AuthService>,
    policy: web::Data<SessionCookiePolicy>,
    body: web::Json<ChangePasswordRequest>,
) -> Result<HttpResponse, AppError> {
    let session = auth
        .change_own_password(&actor, &body.current_password, &body.new_password)
        .await?;

    tracing::info!(user_id = %actor.user_id, "password changed; sessions rotated");

    Ok(session_response(&policy, session))
}

#[derive(Deserialize)]
pub struct ResetPasswordRequest {
    new_password: String,
}

/// Institution-admin reset of someone else's password. The target's
/// sessions are revoked; they sign in again with the new password.
#[post("/api/v1/users/{user_id}/password")]
pub async fn reset_password(
    actor: Actor,
    auth: web::Data<AuthService>,
    target: web::Path<Uuid>,
    body: web::Json<ResetPasswordRequest>,
) -> Result<HttpResponse, AppError> {
    let target_user_id = target.into_inner();
    auth.reset_password(&actor, target_user_id, &body.new_password)
        .await?;

    tracing::info!(
        admin_user_id = %actor.user_id,
        target_user_id = %target_user_id,
        "password reset; target sessions revoked"
    );

    Ok(HttpResponse::NoContent().finish())
}

#[derive(Deserialize)]
pub struct SuspendRequest {
    reason: String,
}

#[post("/api/v1/users/{user_id}/suspend")]
pub async fn suspend_user(
    actor: Actor,
    auth: web::Data<AuthService>,
    target: web::Path<Uuid>,
    body: web::Json<SuspendRequest>,
) -> Result<HttpResponse, AppError> {
    let target_user_id = target.into_inner();
    auth.suspend_user(&actor, target_user_id, &body.reason)
        .await?;

    tracing::info!(
        admin_user_id = %actor.user_id,
        target_user_id = %target_user_id,
        "account suspended; sessions revoked"
    );

    Ok(HttpResponse::NoContent().finish())
}

#[derive(Deserialize)]
pub struct GrantRoleRequest {
    role: String,
}

#[post("/api/v1/users/{user_id}/roles")]
pub async fn grant_role(
    actor: Actor,
    auth: web::Data<AuthService>,
    target: web::Path<Uuid>,
    body: web::Json<GrantRoleRequest>,
) -> Result<HttpResponse, AppError> {
    let role = parse_role(&body.role)?;
    let target_user_id = target.into_inner();
    auth.assign_role(&actor, target_user_id, role).await?;

    tracing::info!(
        admin_user_id = %actor.user_id,
        target_user_id = %target_user_id,
        role = role.code(),
        "role granted; target sessions revoked"
    );

    Ok(HttpResponse::NoContent().finish())
}

#[delete("/api/v1/users/{user_id}/roles/{role_code}")]
pub async fn revoke_role(
    actor: Actor,
    auth: web::Data<AuthService>,
    path: web::Path<(Uuid, String)>,
) -> Result<HttpResponse, AppError> {
    let (target_user_id, role_code) = path.into_inner();
    let role = parse_role(&role_code)?;
    auth.revoke_role(&actor, target_user_id, role).await?;

    tracing::info!(
        admin_user_id = %actor.user_id,
        target_user_id = %target_user_id,
        role = role.code(),
        "role revoked; target sessions revoked"
    );

    Ok(HttpResponse::NoContent().finish())
}

fn parse_role(code: &str) -> Result<Role, AppError> {
    // The set of valid codes is public knowledge (it is in the docs), so
    // echoing an unknown one back is safe and saves a support round-trip.
    Role::from_code(code).ok_or_else(|| AppError::Validation(format!("unknown role: {code}")))
}

pub fn routes(cfg: &mut web::ServiceConfig) {
    cfg.service(login)
        .service(login_page)
        .service(login_form)
        .service(signout_page)
        .service(signout_form)
        .service(logout)
        .service(change_own_password)
        .service(reset_password)
        .service(suspend_user)
        .service(grant_role)
        .service(revoke_role);
}
