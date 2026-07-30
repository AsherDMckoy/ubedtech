//! Theme preference: light / dark / system, persisted in a cookie and
//! stamped onto `<html data-theme=…>` by the SERVER at render time
//! (ADR-14). No inline `<head>` script (the CSP forbids it), no
//! localStorage-first flash of the wrong theme: the attribute is in the
//! first byte of HTML. "system" stamps nothing — the
//! `prefers-color-scheme` block in tokens.css takes over.
//!
//! The rendered chrome needs two per-request values inside `base.html`,
//! which has no template variables of its own: the theme attribute and the
//! CSRF token for the toggle form. Both travel in task-locals scoped by
//! `theme_middleware` (innermost, so the session is already resolved), and
//! Askama reads them through the free functions below — the same calling
//! pattern as `assets::css_href()`, no per-template plumbing.

use actix_web::body::MessageBody;
use actix_web::dev::{ServiceRequest, ServiceResponse};
use actix_web::middleware::Next;
use actix_web::{HttpMessage, HttpRequest, HttpResponse, cookie::Cookie, web};
use serde::Deserialize;

use std::collections::HashSet;

use crate::identity_access::http::SessionCookiePolicy;
use crate::identity_access::sessions::CurrentSession;
use crate::shared::actor::{Actor, Role};
use crate::shared::error::AppError;

pub const THEME_COOKIE: &str = "ub_theme";
pub const RAIL_COOKIE: &str = "ub_rail";

tokio::task_local! {
    static THEME: &'static str;
    static RAIL: &'static str;
    static CSRF: Option<String>;
    static NAV_ROLES: HashSet<Role>;
}

fn cookie_theme(req: &ServiceRequest) -> &'static str {
    match req.cookie(THEME_COOKIE).as_ref().map(Cookie::value) {
        Some("light") => "light",
        Some("dark") => "dark",
        // absent, "system", or garbage: follow the OS
        _ => "system",
    }
}

fn cookie_rail(req: &ServiceRequest) -> &'static str {
    match req.cookie(RAIL_COOKIE).as_ref().map(Cookie::value) {
        Some("collapsed") => "collapsed",
        // absent or garbage: expanded — the JS-off default (ADR-16)
        _ => "expanded",
    }
}

/// Innermost middleware: scope the UI preferences (theme, rail state),
/// the session's CSRF token, and the actor's roles (for the role-aware
/// nav) for the duration of the request, so base.html can render chrome.
pub async fn theme_middleware(
    req: ServiceRequest,
    next: Next<impl MessageBody>,
) -> Result<ServiceResponse<impl MessageBody>, actix_web::Error> {
    let theme = cookie_theme(&req);
    let rail = cookie_rail(&req);
    let csrf = req
        .extensions()
        .get::<CurrentSession>()
        .map(|session| session.csrf_token.clone());
    let roles = req
        .extensions()
        .get::<Actor>()
        .map(|actor| actor.roles.clone())
        .unwrap_or_default();
    THEME
        .scope(
            theme,
            RAIL.scope(
                rail,
                CSRF.scope(csrf, NAV_ROLES.scope(roles, next.call(req))),
            ),
        )
        .await
}

/// The actor's roles, for `shared::nav::items()`; empty when signed out
/// or outside a request.
pub(crate) fn nav_roles() -> HashSet<Role> {
    NAV_ROLES.try_with(Clone::clone).unwrap_or_default()
}

/// `render-pages` runs without a request: render samples with every role
/// so the axe harness sees the full union nav on every shell.
pub fn sample_nav_scope<R>(f: impl FnOnce() -> R) -> R {
    let all = HashSet::from([
        Role::Student,
        Role::Instructor,
        Role::Registrar,
        Role::RecordsOfficer,
        Role::DocumentOfficer,
        Role::InstitutionAdmin,
        Role::PlatformLicensingAdmin,
    ]);
    NAV_ROLES.sync_scope(all, f)
}

/// ` data-theme=dark` / ` data-theme=light` for `<html>`, empty for
/// system. Unquoted attribute value on purpose: Askama HTML-escapes the
/// interpolation, and these values contain nothing escapable.
pub fn html_attr() -> &'static str {
    match THEME.try_with(|theme| *theme) {
        Ok("dark") => " data-theme=dark",
        Ok("light") => " data-theme=light",
        _ => "",
    }
}

/// The current choice, for `aria-pressed` on the toggle buttons.
pub fn current() -> &'static str {
    THEME.try_with(|theme| *theme).unwrap_or("system")
}

/// ` rail-collapsed` for the shell class when the actor collapsed the
/// rail; empty (expanded) otherwise — including JS-off first visits.
pub fn rail_class() -> &'static str {
    match RAIL.try_with(|rail| *rail) {
        Ok("collapsed") => " rail-collapsed",
        _ => "",
    }
}

/// The current rail state, for the toggle button's direction.
pub fn rail_current() -> &'static str {
    RAIL.try_with(|rail| *rail).unwrap_or("expanded")
}

/// CSRF token for the toggle form; empty when signed out (pages without a
/// session render no toggle — the rail is behind sign-in).
pub fn csrf_token() -> String {
    CSRF.try_with(Clone::clone)
        .ok()
        .flatten()
        .unwrap_or_default()
}

#[derive(Deserialize)]
struct ThemeForm {
    theme: String,
    // consumed by the CSRF middleware before the handler runs
    #[serde(rename = "csrf_token")]
    _csrf_token: String,
}

#[derive(Deserialize)]
struct RailForm {
    rail: String,
    #[serde(rename = "csrf_token")]
    _csrf_token: String,
}

/// Set a year-long preference cookie and PRG back to the page the toggle
/// was on. Referer is only a UX nicety (same-origin paths only; anything
/// else falls back home).
fn preference_response(
    request: &HttpRequest,
    policy: &SessionCookiePolicy,
    name: &'static str,
    value: String,
) -> HttpResponse {
    let mut cookie = Cookie::new(name, value);
    cookie.set_path("/");
    cookie.set_same_site(actix_web::cookie::SameSite::Lax);
    cookie.set_secure(policy.secure);
    cookie.set_max_age(actix_web::cookie::time::Duration::days(365));

    let back = request
        .headers()
        .get(actix_web::http::header::REFERER)
        .and_then(|referer| referer.to_str().ok())
        .and_then(|referer| referer.split_once("//")?.1.split_once('/'))
        .map(|(_, path)| format!("/{path}"))
        .unwrap_or_else(|| "/ui/dashboard".to_owned());

    HttpResponse::SeeOther()
        .cookie(cookie)
        .insert_header(("Location", back))
        .finish()
}

/// JS-off path: plain form POST, sets the cookie, redirects back to the
/// page the toggle was on. The enhancement script does the same POST via
/// fetch after flipping data-theme locally.
async fn set_theme(
    request: HttpRequest,
    policy: web::Data<SessionCookiePolicy>,
    form: web::Form<ThemeForm>,
) -> Result<HttpResponse, AppError> {
    let value = match form.theme.as_str() {
        theme @ ("light" | "dark" | "system") => theme.to_owned(),
        _ => return Err(AppError::Validation("unknown theme".into())),
    };
    Ok(preference_response(&request, &policy, THEME_COOKIE, value))
}

/// Rail collapse/expand preference (ADR-16), same shape as the theme.
async fn set_rail(
    request: HttpRequest,
    policy: web::Data<SessionCookiePolicy>,
    form: web::Form<RailForm>,
) -> Result<HttpResponse, AppError> {
    let value = match form.rail.as_str() {
        rail @ ("expanded" | "collapsed") => rail.to_owned(),
        _ => return Err(AppError::Validation("unknown rail state".into())),
    };
    Ok(preference_response(&request, &policy, RAIL_COOKIE, value))
}

pub fn routes(cfg: &mut web::ServiceConfig) {
    cfg.route("/ui/theme", web::post().to(set_theme));
    cfg.route("/ui/rail", web::post().to(set_rail));
}
