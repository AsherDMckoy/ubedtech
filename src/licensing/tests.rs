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
        test_app!($pool, $gate, crate::licensing::ImportVerifyingKey(None))
    };
    ($pool:expr, $gate:expr, $import_key:expr) => {
        actix_test::init_service(
            actix_web::App::new()
                .app_data(web::Data::new($import_key))
                .app_data(web::Data::new($pool.clone()))
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

    // Import is reachable while locked but never anonymous: without a
    // session it answers 401, not 402.
    let import = actix_test::call_service(
        &app,
        actix_test::TestRequest::post()
            .uri("/license/import")
            .set_json(serde_json::json!({
                "format": 1, "claims_json": "{}", "signature_hex": ""
            }))
            .to_request(),
    )
    .await;
    assert_eq!(import.status(), StatusCode::UNAUTHORIZED);

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
    assert_eq!(suspend.status(), StatusCode::SEE_OTHER);

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
    assert_eq!(reactivate.status(), StatusCode::SEE_OTHER);

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

/// The Phase 6 acceptance test: disabling an institution's license denies
/// ordinary access institution-wide, the health/recovery/license-management
/// surface stays reachable, and no individual account is suspended or
/// logged out as a side effect — the lock is the license, not the users.
#[sqlx::test(migrations = "./migrations")]
async fn a_disabled_license_locks_the_institution_but_suspends_nobody(pool: PgPool) {
    let fixture = seed(&pool, "active").await;
    seed_user_with_role(
        &pool,
        fixture.institution_id,
        "plat.admin",
        "pw-admin",
        "platform_licensing_admin",
    )
    .await;
    seed_user_with_role(
        &pool,
        fixture.institution_id,
        "some.student",
        "pw-student",
        "student",
    )
    .await;
    let gate = LicenseGate::new(snapshot(fixture.institution_id, LicenseStatus::Active));
    let app = test_app!(&pool, gate);

    let (admin_cookie, admin_csrf) = login_session!(&app, "plat.admin", "pw-admin");
    let (student_cookie, _student_csrf) = login_session!(&app, "some.student", "pw-student");

    // While active the student reaches protected routes, and the panel
    // renders for the platform admin (and only for them).
    let before = actix_test::call_service(
        &app,
        actix_test::TestRequest::get()
            .uri("/probe/protected")
            .cookie(student_cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(before.status(), StatusCode::OK);
    let panel_as_student = actix_test::call_service(
        &app,
        actix_test::TestRequest::get()
            .uri("/ui/platform/license")
            .cookie(student_cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(panel_as_student.status(), StatusCode::FORBIDDEN);

    // The platform admin disables the license through the panel form.
    let suspend = actix_test::call_service(
        &app,
        actix_test::TestRequest::post()
            .uri(&format!(
                "/ui/platform/institutions/{}/license",
                fixture.institution_id
            ))
            .cookie(admin_cookie.clone())
            .insert_header((
                actix_web::http::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            ))
            .set_payload(format!(
                "status=suspended&reason=contract+lapsed&csrf_token={admin_csrf}"
            ))
            .to_request(),
    )
    .await;
    assert_eq!(suspend.status(), StatusCode::SEE_OTHER);

    // Institution-wide denial: the student's still-valid session now gets
    // 402 (not 401 — they are not logged out, the institution is locked).
    let during = actix_test::call_service(
        &app,
        actix_test::TestRequest::get()
            .uri("/probe/protected")
            .cookie(student_cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(during.status(), StatusCode::PAYMENT_REQUIRED);

    // Health, recovery, and license management all stay reachable.
    for path in ["/health/live", "/health/ready", "/institution-locked"] {
        let response =
            actix_test::call_service(&app, actix_test::TestRequest::get().uri(path).to_request())
                .await;
        assert_eq!(response.status(), StatusCode::OK, "{path} while locked");
    }
    let panel = actix_test::call_service(
        &app,
        actix_test::TestRequest::get()
            .uri("/ui/platform/license")
            .cookie(admin_cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(panel.status(), StatusCode::OK);
    let body = String::from_utf8(actix_test::read_body(panel).await.to_vec()).unwrap();
    crate::shared::assets::assert_page_a11y(&body);
    assert!(body.contains("suspended"));
    assert!(body.contains("contract lapsed"));

    // No account was suspended and no session revoked as a side effect.
    let suspended_users: i64 =
        sqlx::query_scalar("SELECT count(*) FROM user_account WHERE status <> 'active'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(suspended_users, 0);
    let revoked_sessions: i64 =
        sqlx::query_scalar("SELECT count(*) FROM user_session WHERE revoked_at IS NOT NULL")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(revoked_sessions, 0);
}

/// A license is inactive AT the exact `valid_until` instant (`now <
/// valid_until`), and before `valid_from` — the boundary test from the plan.
#[test]
fn the_validity_window_is_half_open() {
    let institution_id = Uuid::new_v4();
    let mut snapshot = snapshot(institution_id, LicenseStatus::Active);

    snapshot.valid_until = Utc::now();
    let gate = LicenseGate::new(snapshot.clone());
    assert!(gate.require_deployment_active().is_err());

    snapshot.valid_until = Utc::now() + Duration::days(1);
    snapshot.valid_from = Utc::now() + Duration::seconds(5);
    let gate = LicenseGate::new(snapshot);
    assert!(gate.require_deployment_active().is_err());
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

/// Signed-license import (self-hosted recovery). The signing key exists
/// ONLY inside this test module — a release binary contains verification
/// code and a public key slot, never signing capability.
mod import {
    use super::*;
    use crate::licensing::ImportVerifyingKey;
    use crate::licensing::signed_license::LicenseClaims;
    use ed25519_dalek::{Signer, SigningKey};

    fn claims(institution_id: Uuid, deployment_id: Uuid) -> LicenseClaims {
        LicenseClaims {
            institution_id,
            deployment_id,
            license_serial: Uuid::new_v4(),
            valid_from: Utc::now() - Duration::days(1),
            valid_until: Utc::now() + Duration::days(365),
            feature_set: serde_json::json!({}),
        }
    }

    fn signed_file(key: &SigningKey, claims: &LicenseClaims) -> serde_json::Value {
        let claims_json = serde_json::to_string(claims).unwrap();
        let signature = key.sign(claims_json.as_bytes());
        serde_json::json!({
            "format": 1,
            "claims_json": claims_json,
            "signature_hex": hex::encode(signature.to_bytes()),
        })
    }

    async fn deployment_id_of(pool: &PgPool, institution_id: Uuid) -> Uuid {
        sqlx::query_scalar(
            "SELECT deployment_id FROM institution_license WHERE institution_id = $1",
        )
        .bind(institution_id)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    macro_rules! import_request {
        ($cookie:expr, $csrf:expr, $file:expr) => {
            actix_test::TestRequest::post()
                .uri("/license/import")
                .cookie($cookie.clone())
                .insert_header((crate::identity_access::csrf::CSRF_HEADER, $csrf.as_str()))
                .set_json($file)
                .to_request()
        };
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn a_signed_license_import_unlocks_a_locked_deployment(pool: PgPool) {
        let fixture = seed(&pool, "suspended").await;
        let admin_id = seed_user_with_role(
            &pool,
            fixture.institution_id,
            "inst.admin",
            "pw-admin",
            "institution_admin",
        )
        .await;
        let signing_key = SigningKey::from_bytes(&[7u8; 32]);
        let gate = LicenseGate::new(snapshot(fixture.institution_id, LicenseStatus::Suspended));
        let app = test_app!(
            &pool,
            gate,
            ImportVerifyingKey(Some(signing_key.verifying_key()))
        );

        let (cookie, csrf) = login_session!(&app, "inst.admin", "pw-admin");

        // Locked before the import.
        let before = actix_test::call_service(
            &app,
            actix_test::TestRequest::get()
                .uri("/probe/protected")
                .cookie(cookie.clone())
                .to_request(),
        )
        .await;
        assert_eq!(before.status(), StatusCode::PAYMENT_REQUIRED);

        let deployment_id = deployment_id_of(&pool, fixture.institution_id).await;
        let file = signed_file(&signing_key, &claims(fixture.institution_id, deployment_id));
        let imported = actix_test::call_service(&app, import_request!(cookie, csrf, &file)).await;
        assert_eq!(imported.status(), StatusCode::OK);
        let body: serde_json::Value = actix_test::read_body_json(imported).await;
        assert_eq!(body["status"], "active");
        assert_eq!(body["version"], 2);

        // The gate swapped after commit: the deployment is open again.
        let after = actix_test::call_service(
            &app,
            actix_test::TestRequest::get()
                .uri("/probe/protected")
                .cookie(cookie.clone())
                .to_request(),
        )
        .await;
        assert_eq!(after.status(), StatusCode::OK);

        // Durable state, change record, and audit all landed together.
        let status: String =
            sqlx::query_scalar("SELECT status FROM institution_license WHERE institution_id = $1")
                .bind(fixture.institution_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "active");
        let change_reason: String =
            sqlx::query_scalar("SELECT reason FROM license_change WHERE institution_id = $1")
                .bind(fixture.institution_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(change_reason.starts_with("signed license import"));
        let audits: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM audit_event WHERE institution_id = $1 \
             AND action = 'license.imported' AND actor_user_id = $2",
        )
        .bind(fixture.institution_id)
        .bind(admin_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(audits, 1);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn bad_or_misdirected_license_files_are_rejected(pool: PgPool) {
        let fixture = seed(&pool, "suspended").await;
        seed_user_with_role(
            &pool,
            fixture.institution_id,
            "inst.admin",
            "pw-admin",
            "institution_admin",
        )
        .await;
        seed_user_with_role(
            &pool,
            fixture.institution_id,
            "some.student",
            "pw-student",
            "student",
        )
        .await;
        let signing_key = SigningKey::from_bytes(&[7u8; 32]);
        let wrong_key = SigningKey::from_bytes(&[9u8; 32]);
        let gate = LicenseGate::new(snapshot(fixture.institution_id, LicenseStatus::Suspended));
        let app = test_app!(
            &pool,
            gate.clone(),
            ImportVerifyingKey(Some(signing_key.verifying_key()))
        );

        let (admin_cookie, admin_csrf) = login_session!(&app, "inst.admin", "pw-admin");
        let (student_cookie, student_csrf) = login_session!(&app, "some.student", "pw-student");
        let deployment_id = deployment_id_of(&pool, fixture.institution_id).await;
        let good_claims = claims(fixture.institution_id, deployment_id);
        let good_file = signed_file(&signing_key, &good_claims);

        // A student cannot import even a perfectly valid file.
        let as_student = actix_test::call_service(
            &app,
            import_request!(student_cookie, student_csrf, &good_file),
        )
        .await;
        assert_eq!(as_student.status(), StatusCode::FORBIDDEN);

        // Tampered claims, a foreign signature, a wrong deployment, an
        // expired window, a misdirected institution, an unknown format.
        let mut tampered = good_file.clone();
        tampered["claims_json"] = serde_json::Value::String(
            good_file["claims_json"]
                .as_str()
                .unwrap()
                .replace("2026", "2027"),
        );
        let foreign_signature = signed_file(&wrong_key, &good_claims);
        let wrong_deployment = signed_file(
            &signing_key,
            &claims(fixture.institution_id, Uuid::new_v4()),
        );
        let mut expired_claims = claims(fixture.institution_id, deployment_id);
        expired_claims.valid_from = Utc::now() - Duration::days(730);
        expired_claims.valid_until = Utc::now() - Duration::days(365);
        let expired = signed_file(&signing_key, &expired_claims);
        let misdirected = signed_file(&signing_key, &claims(Uuid::new_v4(), deployment_id));
        let mut unknown_format = good_file.clone();
        unknown_format["format"] = serde_json::json!(2);

        for (label, file) in [
            ("tampered", &tampered),
            ("foreign signature", &foreign_signature),
            ("wrong deployment", &wrong_deployment),
            ("expired", &expired),
            ("misdirected institution", &misdirected),
            ("unknown format", &unknown_format),
        ] {
            let response =
                actix_test::call_service(&app, import_request!(admin_cookie, admin_csrf, file))
                    .await;
            assert_eq!(
                response.status(),
                StatusCode::UNPROCESSABLE_ENTITY,
                "{label} must be rejected"
            );
        }

        // Nothing was accepted: still locked, no change records.
        assert!(gate.require_deployment_active().is_err());
        let changes: i64 = sqlx::query_scalar("SELECT count(*) FROM license_change")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(changes, 0);

        // And a deployment with no configured key refuses even a valid file.
        let keyless = test_app!(
            &pool,
            LicenseGate::new(snapshot(fixture.institution_id, LicenseStatus::Suspended))
        );
        let (cookie, csrf) = login_session!(&keyless, "inst.admin", "pw-admin");
        let refused =
            actix_test::call_service(&keyless, import_request!(cookie, csrf, &good_file)).await;
        assert_eq!(refused.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
}
