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
    static USER_MENU: Option<UserMenu>;
}

/// Identity shown by the persistent header's account menu — the fields
/// the system actually models (there is no faculty or class-level on
/// people; docs/IMPLEMENTATION_PLAN.md assumption A39). Fetched once per
/// signed-in `/ui/` page render, one indexed LEFT-JOIN query.
#[derive(Clone, sqlx::FromRow)]
pub struct UserMenu {
    pub full_name: String,
    pub username: String,
    pub email: String,
    pub student_number: Option<String>,
    pub program_code: Option<String>,
    pub academic_status: Option<String>,
}

impl UserMenu {
    /// Display name: the full name when set, the username until then.
    pub fn name(&self) -> &str {
        if self.full_name.is_empty() {
            &self.username
        } else {
            &self.full_name
        }
    }

    /// Up to two initials for the avatar chip ("Dana Castillo" → "DC",
    /// "demo.student" → "DS").
    pub fn initials(&self) -> String {
        self.name()
            .split([' ', '.', '-'])
            .filter(|part| !part.is_empty())
            .take(2)
            .filter_map(|part| part.chars().next())
            .flat_map(char::to_uppercase)
            .collect()
    }

    /// "good_standing" → "Good standing".
    pub fn standing(&self) -> Option<String> {
        self.academic_status.as_ref().map(|status| {
            let mut label = status.replace('_', " ");
            if let Some(first) = label.get_mut(0..1) {
                first.make_ascii_uppercase();
            }
            label
        })
    }
}

/// The signed-in identity for the header menu; None when signed out or
/// outside a request.
pub fn user_menu() -> Option<UserMenu> {
    USER_MENU.try_with(Clone::clone).ok().flatten()
}

/// Human labels for the actor's roles, stable order — the menu's answer
/// for non-student accounts.
pub fn role_labels() -> Vec<&'static str> {
    const ORDER: [(Role, &str); 7] = [
        (Role::Student, "Student"),
        (Role::Instructor, "Instructor"),
        (Role::Registrar, "Registrar"),
        (Role::RecordsOfficer, "Records officer"),
        (Role::DocumentOfficer, "Document officer"),
        (Role::InstitutionAdmin, "Institution admin"),
        (Role::PlatformLicensingAdmin, "Platform licensing"),
    ];
    let roles = nav_roles();
    ORDER
        .into_iter()
        .filter(|(role, _)| roles.contains(role))
        .map(|(_, label)| label)
        .collect()
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
    let actor = req.extensions().get::<Actor>().cloned();
    let roles = actor
        .as_ref()
        .map(|actor| actor.roles.clone())
        .unwrap_or_default();
    // Header identity: UI pages only (assets/health/API skip the query).
    // Chrome is decorative — a failed lookup logs and renders no menu
    // rather than failing the page.
    let menu = match actor {
        Some(actor) if req.path().starts_with("/ui/") => {
            match req.app_data::<web::Data<sqlx::PgPool>>() {
                Some(pool) => fetch_user_menu(pool, actor.user_id)
                    .await
                    .unwrap_or_else(|error| {
                        tracing::warn!(%error, "header identity lookup failed");
                        None
                    }),
                None => None,
            }
        }
        _ => None,
    };
    THEME
        .scope(
            theme,
            RAIL.scope(
                rail,
                CSRF.scope(
                    csrf,
                    NAV_ROLES.scope(roles, USER_MENU.scope(menu, next.call(req))),
                ),
            ),
        )
        .await
}

async fn fetch_user_menu(
    pool: &sqlx::PgPool,
    user_id: uuid::Uuid,
) -> Result<Option<UserMenu>, sqlx::Error> {
    sqlx::query_as::<_, UserMenu>(
        "SELECT ua.full_name, ua.username, ua.email, \
                sp.student_number, sp.program_code, sp.academic_status \
           FROM user_account ua \
           LEFT JOIN student_profile sp \
             ON sp.user_id = ua.id AND sp.institution_id = ua.institution_id \
          WHERE ua.id = $1",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await
}

/// The actor's roles, for `shared::nav::items()`; empty when signed out
/// or outside a request.
pub(crate) fn nav_roles() -> HashSet<Role> {
    NAV_ROLES.try_with(Clone::clone).unwrap_or_default()
}

/// `render-pages` runs without a request: render samples with every role
/// so the axe harness sees the full union nav on every shell, and with a
/// dummy CSRF token so the session-only chrome (theme toggle, rail
/// toggle) renders into the fixtures the jsdom behavior tests drive.
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
    let sample_user = UserMenu {
        full_name: "Dana Castillo".into(),
        username: "demo.student".into(),
        email: "demo.student@example.test".into(),
        student_number: Some("2023-1187".into()),
        program_code: Some("BSC-CS".into()),
        academic_status: Some("good_standing".into()),
    };
    CSRF.sync_scope(Some("sample-fixture-token".into()), || {
        NAV_ROLES.sync_scope(all, || USER_MENU.sync_scope(Some(sample_user), f))
    })
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
