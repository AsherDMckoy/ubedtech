//! Platform licensing admin HTTP adapters. These live on `/ui/platform/`,
//! which the license middleware exempts: the whole point of this surface is
//! recovering a locked deployment, so it must stay reachable while locked.
//! Authorization is the handlers' own job (platform licensing admin only).

use crate::identity_access::sessions::CurrentSession;
use crate::shared::{
    actor::{Actor, Role},
    error::AppError,
};
use actix_web::http::StatusCode;
use actix_web::{HttpResponse, get, post, web};
use askama::Template;
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::licensing::signed_license::SignedLicenseFile;
use crate::licensing::{ImportVerifyingKey, LicenseGate, LicenseService, LicenseStatus};

/// Self-hosted recovery: import a platform-signed license file. License-
/// exempt (reachable while locked) but not anonymous — an institution or
/// platform admin signs in first (login is also exempt), so the audit trail
/// has a real actor. The signature check is the actual authority.
#[post("/license/import")]
pub async fn import_license(
    actor: Actor,
    service: web::Data<LicenseService>,
    key: web::Data<ImportVerifyingKey>,
    body: web::Json<SignedLicenseFile>,
) -> Result<HttpResponse, AppError> {
    let snapshot = service.import_signed(&actor, key.0.as_ref(), &body).await?;
    Ok(HttpResponse::Ok().json(serde_json::json!({
        "status": snapshot.status,
        "valid_from": snapshot.valid_from,
        "valid_until": snapshot.valid_until,
        "version": snapshot.version,
    })))
}

struct ChangeView {
    changed_at: String,
    old_status: String,
    new_status: String,
    reason: String,
    changed_by: String,
}

#[derive(Template)]
#[template(path = "pages/license_panel.html")]
struct LicensePanelPage<'a> {
    csrf_token: &'a str,
    institution_id: Uuid,
    status: &'a str,
    valid_from: String,
    valid_until: String,
    version: i64,
    changes: Vec<ChangeView>,
    notice: Option<&'a str>,
    error: Option<&'a str>,
}

#[derive(Deserialize)]
pub struct NoticeQuery {
    notice: Option<String>,
}

#[get("/ui/platform/license")]
pub async fn license_panel_page(
    actor: Actor,
    current: CurrentSession,
    gate: web::Data<LicenseGate>,
    pool: web::Data<PgPool>,
    query: web::Query<NoticeQuery>,
) -> Result<HttpResponse, AppError> {
    let notice = match query.notice.as_deref() {
        Some("updated") => Some("License status updated."),
        _ => None,
    };
    let body = render_panel(&actor, &current, &gate, &pool, notice, None).await?;
    Ok(html(StatusCode::OK, body))
}

#[derive(Deserialize)]
pub struct LicenseStatusForm {
    status: String,
    reason: String,
    // Consumed by the CSRF middleware; kept so the form deserializes.
    #[serde(rename = "csrf_token")]
    _csrf_token: String,
}

#[post("/ui/platform/institutions/{institution_id}/license")]
pub async fn change_license_form(
    actor: Actor,
    current: CurrentSession,
    service: web::Data<LicenseService>,
    gate: web::Data<LicenseGate>,
    pool: web::Data<PgPool>,
    institution_id: web::Path<Uuid>,
    form: web::Form<LicenseStatusForm>,
) -> Result<HttpResponse, AppError> {
    let status = match form.status.as_str() {
        "active" => Some(LicenseStatus::Active),
        "suspended" => Some(LicenseStatus::Suspended),
        "expired" => Some(LicenseStatus::Expired),
        _ => None,
    };

    let outcome = match status {
        Some(status) => service
            .set_status(&actor, institution_id.into_inner(), status, &form.reason)
            .await
            .map(|_| ()),
        None => Err(AppError::Validation("unknown license status".into())),
    };

    match outcome {
        Ok(()) => Ok(see_other("/ui/platform/license?notice=updated")),
        Err(AppError::Validation(message)) => {
            let body = render_panel(&actor, &current, &gate, &pool, None, Some(&message)).await?;
            Ok(html(StatusCode::UNPROCESSABLE_ENTITY, body))
        }
        Err(other) => Err(other),
    }
}

async fn render_panel(
    actor: &Actor,
    current: &CurrentSession,
    gate: &LicenseGate,
    pool: &PgPool,
    notice: Option<&str>,
    error: Option<&str>,
) -> Result<String, AppError> {
    // Reading the license panel is as privileged as changing it.
    if !actor.has_role(Role::PlatformLicensingAdmin) {
        return Err(AppError::Forbidden);
    }

    let snapshot = gate.snapshot();

    #[derive(sqlx::FromRow)]
    struct ChangeRow {
        changed_at: chrono::DateTime<chrono::Utc>,
        old_status: String,
        new_status: String,
        reason: String,
        changed_by: String,
    }

    let changes = sqlx::query_as::<_, ChangeRow>(
        r#"
        SELECT lc.changed_at, lc.old_status, lc.new_status, lc.reason,
               ua.username AS changed_by
        FROM license_change lc
        JOIN user_account ua ON ua.id = lc.changed_by_user_id
        WHERE lc.institution_id = $1
        ORDER BY lc.changed_at DESC
        LIMIT 20
        "#,
    )
    .bind(snapshot.institution_id)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|row| ChangeView {
        changed_at: row.changed_at.format("%Y-%m-%d %H:%M UTC").to_string(),
        old_status: row.old_status,
        new_status: row.new_status,
        reason: row.reason,
        changed_by: row.changed_by,
    })
    .collect();

    Ok(LicensePanelPage {
        csrf_token: &current.csrf_token,
        institution_id: snapshot.institution_id,
        status: match snapshot.status {
            LicenseStatus::Active => "active",
            LicenseStatus::Suspended => "suspended",
            LicenseStatus::Expired => "expired",
        },
        valid_from: snapshot.valid_from.format("%Y-%m-%d %H:%M UTC").to_string(),
        valid_until: snapshot
            .valid_until
            .format("%Y-%m-%d %H:%M UTC")
            .to_string(),
        version: snapshot.version,
        changes,
        notice,
        error,
    }
    .render()?)
}

fn html(status: StatusCode, body: String) -> HttpResponse {
    HttpResponse::build(status)
        .content_type("text/html; charset=utf-8")
        .body(body)
}

fn see_other(location: &str) -> HttpResponse {
    HttpResponse::SeeOther()
        .insert_header(("Location", location.to_owned()))
        .finish()
}

pub fn routes(cfg: &mut web::ServiceConfig) {
    cfg.service(license_panel_page)
        .service(change_license_form)
        .service(import_license);
}
