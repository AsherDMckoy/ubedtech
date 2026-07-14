//! HTTP acceptance tests for the license gate: a locked institution answers
//! 402 on protected routes while the recovery surface stays reachable, and a
//! platform licensing admin can flip the license over HTTP end to end.

use actix_web::http::StatusCode;
use actix_web::{HttpResponse, get, test as actix_test, web};
use chrono::{Duration, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::audit::AuditWriter;
use crate::identity_access::http::SessionCookiePolicy;
use crate::identity_access::password::PasswordService;
use crate::identity_access::service::AuthService;
use crate::identity_access::sessions::SessionService;
use crate::licensing::{LicenseGate, LicenseService, LicenseSnapshot, LicenseStatus};
use crate::shared::actor::Actor;

fn snapshot(institution_id: Uuid, status: LicenseStatus) -> LicenseSnapshot {
    LicenseSnapshot {
        institution_id,
        deployment_id: Uuid::new_v4(),
        status,
        valid_from: Utc::now() - Duration::days(1),
        valid_until: Utc::now() + Duration::days(365),
        version: 1,
        feature_set: serde_json::json!({}),
    }
}

/// Stand-in for any protected route; the license gate must reject it while
/// locked before it is ever reached.
#[get("/probe/protected")]
async fn protected_probe(_actor: Actor) -> HttpResponse {
    HttpResponse::Ok().body("reached")
}

macro_rules! test_app {
    ($pool:expr, $gate:expr) => {
        actix_test::init_service(
            actix_web::App::new()
                .app_data(web::Data::new(SessionService::new(
                    $pool.clone(),
                    1800,
                    43200,
                )))
                .app_data(web::Data::new(
                    AuthService::new(
                        $pool.clone(),
                        PasswordService::new(8, 1, 1).unwrap(),
                        SessionService::new($pool.clone(), 1800, 43200),
                        AuditWriter,
                        10,
                        900,
                    )
                    .unwrap(),
                ))
                .app_data(web::Data::new($gate.clone()))
                .app_data(web::Data::new(LicenseService::new(
                    $pool.clone(),
                    $gate.clone(),
                    AuditWriter,
                )))
                .app_data(web::Data::new(SessionCookiePolicy {
                    secure: false,
                    max_age_secs: 43200,
                }))
                .app_data(web::Data::new(crate::app::Readiness::new(true)))
                // Same order as main.rs.
                .wrap(actix_web::middleware::from_fn(
                    crate::identity_access::csrf::csrf_middleware,
                ))
                .wrap(actix_web::middleware::from_fn(
                    crate::identity_access::middleware::session_middleware,
                ))
                .wrap(actix_web::middleware::from_fn(
                    crate::licensing::middleware::license_middleware,
                ))
                .wrap(actix_web::middleware::NormalizePath::trim())
                .configure(crate::app::recovery_routes)
                .service(protected_probe),
        )
        .await
    };
}

macro_rules! login_session {
    ($app:expr, $user:expr, $pw:expr) => {{
        let response = actix_test::call_service(
            $app,
            actix_test::TestRequest::post()
                .uri("/api/v1/session/login")
                .peer_addr("127.0.0.1:9999".parse().unwrap())
                .set_json(serde_json::json!({ "username": $user, "password": $pw }))
                .to_request(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK, "login must succeed");
        let cookie = response
            .response()
            .cookies()
            .find(|cookie| cookie.name() == "ub_session")
            .expect("session cookie")
            .into_owned();
        let body: serde_json::Value = actix_test::read_body_json(response).await;
        let csrf = body["csrf_token"].as_str().unwrap().to_owned();
        (cookie, csrf)
    }};
}

struct Fixture {
    institution_id: Uuid,
}

async fn seed(pool: &PgPool, license_status: &str) -> Fixture {
    let institution_id = Uuid::new_v4();
    sqlx::query("INSERT INTO institution (id, code, name) VALUES ($1, $2, 'License U')")
        .bind(institution_id)
        .bind(format!("L-{}", &institution_id.to_string()[..8]))
        .execute(pool)
        .await
        .unwrap();

    sqlx::query(
        "INSERT INTO institution_license (institution_id, deployment_id, mode, status, \
         valid_from, valid_until, feature_set, version) \
         VALUES ($1, $2, 'hosted', $3, now() - interval '1 day', \
         now() + interval '365 days', '{}', 1)",
    )
    .bind(institution_id)
    .bind(Uuid::new_v4())
    .bind(license_status)
    .execute(pool)
    .await
    .unwrap();

    Fixture { institution_id }
}

async fn seed_user_with_role(
    pool: &PgPool,
    institution_id: Uuid,
    username: &str,
    password: &str,
    role_code: &str,
) -> Uuid {
    let user_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO user_account (id, institution_id, username, email) VALUES ($1, $2, $3, $4)",
    )
    .bind(user_id)
    .bind(institution_id)
    .bind(username)
    .bind(format!("{username}@test.invalid"))
    .execute(pool)
    .await
    .unwrap();

    let hash = PasswordService::new(8, 1, 1)
        .unwrap()
        .hash(password)
        .unwrap();
    sqlx::query("INSERT INTO password_credential (user_id, password_hash) VALUES ($1, $2)")
        .bind(user_id)
        .bind(hash)
        .execute(pool)
        .await
        .unwrap();

    sqlx::query(
        "INSERT INTO user_role (institution_id, user_id, role_id) \
         SELECT $1, $2, id FROM role WHERE code = $3",
    )
    .bind(institution_id)
    .bind(user_id)
    .bind(role_code)
    .execute(pool)
    .await
    .unwrap();

    user_id
}

#[sqlx::test(migrations = "./migrations")]
async fn locked_institution_answers_402_and_recovery_stays_reachable(pool: PgPool) {
    let fixture = seed(&pool, "suspended").await;
    seed_user_with_role(
        &pool,
        fixture.institution_id,
        "plat.admin",
        "pw-admin",
        "platform_licensing_admin",
    )
    .await;
    let gate = LicenseGate::new(snapshot(fixture.institution_id, LicenseStatus::Suspended));
    let app = test_app!(&pool, gate);

    // Protected surface: 402, even before authentication.
    for (method, path) in [
        ("GET", "/probe/protected"),
        ("POST", "/ui/registration/add"),
    ] {
        let request = match method {
            "GET" => actix_test::TestRequest::get(),
            _ => actix_test::TestRequest::post(),
        }
        .uri(path)
        .to_request();
        let response = actix_test::call_service(&app, request).await;
        assert_eq!(
            response.status(),
            StatusCode::PAYMENT_REQUIRED,
            "{method} {path} while locked"
        );
    }

    // Recovery surface: all reachable.
    for path in ["/health/live", "/health/ready", "/institution-locked"] {
        let response =
            actix_test::call_service(&app, actix_test::TestRequest::get().uri(path).to_request())
                .await;
        assert_eq!(response.status(), StatusCode::OK, "{path} while locked");
    }

    let status = actix_test::call_service(
        &app,
        actix_test::TestRequest::get()
            .uri("/license/status")
            .to_request(),
    )
    .await;
    assert_eq!(status.status(), StatusCode::OK);
    let body: serde_json::Value = actix_test::read_body_json(status).await;
    assert_eq!(body["status"], "suspended");

    let import = actix_test::call_service(
        &app,
        actix_test::TestRequest::post()
            .uri("/license/import")
            .to_request(),
    )
    .await;
    assert_eq!(import.status(), StatusCode::NOT_IMPLEMENTED);

    // And crucially: login still works while locked, so an operator can get in.
    let (_cookie, _csrf) = login_session!(&app, "plat.admin", "pw-admin");
}

#[sqlx::test(migrations = "./migrations")]
async fn platform_admin_flips_the_license_end_to_end(pool: PgPool) {
    let fixture = seed(&pool, "active").await;
    let admin_id = seed_user_with_role(
        &pool,
        fixture.institution_id,
        "plat.admin",
        "pw-admin",
        "platform_licensing_admin",
    )
    .await;
    let gate = LicenseGate::new(snapshot(fixture.institution_id, LicenseStatus::Active));
    let app = test_app!(&pool, gate);

    let (cookie, csrf) = login_session!(&app, "plat.admin", "pw-admin");

    // Active: protected route reachable.
    let before = actix_test::call_service(
        &app,
        actix_test::TestRequest::get()
            .uri("/probe/protected")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(before.status(), StatusCode::OK);

    // Suspend over HTTP (exempt path; CSRF via form field; role-guarded).
    let suspend = actix_test::call_service(
        &app,
        actix_test::TestRequest::post()
            .uri(&format!(
                "/ui/platform/institutions/{}/license",
                fixture.institution_id
            ))
            .cookie(cookie.clone())
            .insert_header((
                actix_web::http::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            ))
            .set_payload(format!(
                "status=suspended&reason=nonpayment&csrf_token={csrf}"
            ))
            .to_request(),
    )
    .await;
    assert_eq!(suspend.status(), StatusCode::OK);

    // Locked: the same protected route now answers 402.
    let during = actix_test::call_service(
        &app,
        actix_test::TestRequest::get()
            .uri("/probe/protected")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(during.status(), StatusCode::PAYMENT_REQUIRED);

    // The change and its audit record were written in the same transaction.
    let change_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM license_change WHERE institution_id = $1")
            .bind(fixture.institution_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(change_count, 1);
    let audit_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_event WHERE institution_id = $1 \
         AND action = 'license.status_changed' AND actor_user_id = $2",
    )
    .bind(fixture.institution_id)
    .bind(admin_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(audit_count, 1);

    // Reactivate — the admin's session and the license routes still work.
    let reactivate = actix_test::call_service(
        &app,
        actix_test::TestRequest::post()
            .uri(&format!(
                "/ui/platform/institutions/{}/license",
                fixture.institution_id
            ))
            .cookie(cookie.clone())
            .insert_header((
                actix_web::http::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            ))
            .set_payload(format!(
                "status=active&reason=payment+received&csrf_token={csrf}"
            ))
            .to_request(),
    )
    .await;
    assert_eq!(reactivate.status(), StatusCode::OK);

    let after = actix_test::call_service(
        &app,
        actix_test::TestRequest::get()
            .uri("/probe/protected")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(after.status(), StatusCode::OK);
}

#[sqlx::test(migrations = "./migrations")]
async fn non_platform_roles_cannot_touch_the_license(pool: PgPool) {
    let fixture = seed(&pool, "active").await;
    seed_user_with_role(
        &pool,
        fixture.institution_id,
        "inst.admin",
        "pw-inst",
        "institution_admin",
    )
    .await;
    let gate = LicenseGate::new(snapshot(fixture.institution_id, LicenseStatus::Active));
    let app = test_app!(&pool, gate);

    let (cookie, csrf) = login_session!(&app, "inst.admin", "pw-inst");

    // Even an institution admin is not the platform operator.
    let attempt = actix_test::call_service(
        &app,
        actix_test::TestRequest::post()
            .uri(&format!(
                "/ui/platform/institutions/{}/license",
                fixture.institution_id
            ))
            .cookie(cookie.clone())
            .insert_header((
                actix_web::http::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            ))
            .set_payload(format!("status=suspended&reason=mine&csrf_token={csrf}"))
            .to_request(),
    )
    .await;
    assert_eq!(attempt.status(), StatusCode::FORBIDDEN);

    // Nothing changed: the deployment is still active.
    let still_ok = actix_test::call_service(
        &app,
        actix_test::TestRequest::get()
            .uri("/probe/protected")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(still_ok.status(), StatusCode::OK);
}
