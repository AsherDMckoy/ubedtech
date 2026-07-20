//! HTTP integration tests for the session lifecycle: the acceptance proof
//! that requests without a valid session get 401 and requests with one reach
//! the handler with the correct Actor.

use actix_web::http::StatusCode;
use actix_web::{HttpResponse, get, test as actix_test, web};
use chrono::{Duration, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::identity_access::http::SessionCookiePolicy;
use crate::identity_access::password::PasswordService;
use crate::identity_access::service::AuthService;
use crate::identity_access::sessions::SessionService;
use crate::licensing::{LicenseGate, LicenseSnapshot, LicenseStatus};
use crate::shared::actor::Actor;

const TEST_MAX_FAILURES: u32 = 3;

fn sessions_svc(pool: &PgPool) -> SessionService {
    SessionService::new(pool.clone(), 1800, 43200)
}

fn auth_svc(pool: &PgPool) -> AuthService {
    AuthService::new(
        pool.clone(),
        PasswordService::new(8, 1, 1).unwrap(),
        sessions_svc(pool),
        crate::audit::AuditWriter,
        TEST_MAX_FAILURES,
        900,
    )
    .unwrap()
}

fn gate(institution_id: Uuid) -> LicenseGate {
    LicenseGate::new(LicenseSnapshot {
        institution_id,
        deployment_id: Uuid::new_v4(),
        status: LicenseStatus::Active,
        valid_from: Utc::now() - Duration::days(1),
        valid_until: Utc::now() + Duration::days(365),
        version: 1,
        feature_set: serde_json::json!({}),
    })
}

/// Test-only handler that echoes the resolved Actor, proving the middleware
/// populated request extensions with the right identity.
#[get("/probe/whoami")]
async fn whoami(actor: Actor) -> HttpResponse {
    let mut roles: Vec<&str> = actor.roles.iter().map(|role| role.code()).collect();
    roles.sort_unstable();
    HttpResponse::Ok().json(serde_json::json!({
        "user_id": actor.user_id,
        "institution_id": actor.institution_id,
        "student_id": actor.student_id,
        "roles": roles,
    }))
}

#[derive(serde::Deserialize)]
struct FormProbeBody {
    // Present so the CSRF middleware's re-injected body still deserializes;
    // validation happened in the middleware.
    #[serde(rename = "csrf_token")]
    _csrf_token: String,
    note: String,
}

/// Test-only form endpoint proving the CSRF middleware buffers, checks, and
/// re-injects urlencoded bodies without breaking handler extraction.
#[actix_web::post("/probe/form")]
async fn form_probe(_actor: Actor, form: web::Form<FormProbeBody>) -> HttpResponse {
    HttpResponse::Ok().body(format!("noted:{}", form.note))
}

macro_rules! test_app {
    ($pool:expr, $institution:expr) => {
        actix_test::init_service(
            actix_web::App::new()
                .app_data(web::Data::new(sessions_svc($pool)))
                .app_data(web::Data::new(auth_svc($pool)))
                .app_data(web::Data::new(gate($institution)))
                .app_data(web::Data::new(SessionCookiePolicy {
                    secure: false,
                    max_age_secs: 43200,
                }))
                // Same order as main.rs: csrf innermost (needs the session
                // already resolved), then session resolution.
                .wrap(actix_web::middleware::from_fn(
                    crate::identity_access::csrf::csrf_middleware,
                ))
                .wrap(actix_web::middleware::from_fn(
                    crate::identity_access::middleware::session_middleware,
                ))
                // Same as main.rs: signed-out browsers land on sign-in.
                .wrap(crate::app::login_redirects())
                .configure(crate::identity_access::http::routes)
                .service(whoami)
                .service(form_probe),
        )
        .await
    };
}

/// Login over HTTP, returning the session cookie and the CSRF token.
macro_rules! login_session {
    ($app:expr, $user:expr, $pw:expr) => {{
        let response = actix_test::call_service($app, login_request($user, $pw).to_request()).await;
        assert_eq!(response.status(), StatusCode::OK);
        let cookie = session_cookie_from(&response);
        let body: serde_json::Value = actix_test::read_body_json(response).await;
        let csrf = body["csrf_token"].as_str().unwrap().to_owned();
        (cookie, csrf)
    }};
}

async fn seed_institution(pool: &PgPool) -> Uuid {
    let institution_id = Uuid::new_v4();
    sqlx::query("INSERT INTO institution (id, code, name) VALUES ($1, $2, 'Auth U')")
        .bind(institution_id)
        .bind(format!("A-{}", &institution_id.to_string()[..8]))
        .execute(pool)
        .await
        .unwrap();
    institution_id
}

async fn seed_credentialed_user(
    pool: &PgPool,
    institution_id: Uuid,
    username: &str,
    password: &str,
    status: &str,
) -> Uuid {
    let user_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO user_account (id, institution_id, username, email, status) \
         VALUES ($1, $2, $3, $4, $5::user_status)",
    )
    .bind(user_id)
    .bind(institution_id)
    .bind(username)
    .bind(format!("{username}@test.invalid"))
    .bind(status)
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

    user_id
}

async fn assign_role(pool: &PgPool, institution_id: Uuid, user_id: Uuid, role_code: &str) {
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
}

fn login_request(username: &str, password: &str) -> actix_test::TestRequest {
    actix_test::TestRequest::post()
        .uri("/api/v1/session/login")
        .peer_addr("127.0.0.1:9999".parse().unwrap())
        .set_json(serde_json::json!({ "username": username, "password": password }))
}

fn session_cookie_from(
    response: &actix_web::dev::ServiceResponse<impl actix_web::body::MessageBody>,
) -> actix_web::cookie::Cookie<'static> {
    response
        .response()
        .cookies()
        .find(|cookie| cookie.name() == "ub_session")
        .expect("login response sets the session cookie")
        .into_owned()
}

#[sqlx::test(migrations = "./migrations")]
async fn request_without_a_session_is_401(pool: PgPool) {
    let institution_id = seed_institution(&pool).await;
    let app = test_app!(&pool, institution_id);

    let response = actix_test::call_service(
        &app,
        actix_test::TestRequest::get()
            .uri("/probe/whoami")
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "./migrations")]
async fn garbage_and_forged_cookies_are_401(pool: PgPool) {
    let institution_id = seed_institution(&pool).await;
    let app = test_app!(&pool, institution_id);

    for value in ["garbage", &"a".repeat(64), ""] {
        let response = actix_test::call_service(
            &app,
            actix_test::TestRequest::get()
                .uri("/probe/whoami")
                .cookie(actix_web::cookie::Cookie::new("ub_session", value))
                .to_request(),
        )
        .await;
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "value {value:?}"
        );
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn valid_session_reaches_the_handler_with_the_correct_actor(pool: PgPool) {
    let institution_id = seed_institution(&pool).await;
    let user_id =
        seed_credentialed_user(&pool, institution_id, "student.one", "hunter2!", "active").await;

    let student_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO student_profile (id, institution_id, user_id, student_number, \
         program_code) VALUES ($1, $2, $3, 'S-100', 'CS')",
    )
    .bind(student_id)
    .bind(institution_id)
    .bind(user_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO user_role (institution_id, user_id, role_id) \
         SELECT $1, $2, id FROM role WHERE code = 'student'",
    )
    .bind(institution_id)
    .bind(user_id)
    .execute(&pool)
    .await
    .unwrap();

    let app = test_app!(&pool, institution_id);

    let login =
        actix_test::call_service(&app, login_request("student.one", "hunter2!").to_request()).await;
    assert_eq!(login.status(), StatusCode::OK);

    let cookie = session_cookie_from(&login);
    // Cookie hardening: HttpOnly, SameSite=Lax, path-wide, bounded lifetime.
    assert_eq!(cookie.http_only(), Some(true));
    assert_eq!(cookie.same_site(), Some(actix_web::cookie::SameSite::Lax));
    assert_eq!(cookie.path(), Some("/"));
    assert!(cookie.max_age().is_some());
    assert_ne!(cookie.secure(), Some(true), "Secure is production-only");

    let body: serde_json::Value = actix_test::read_body_json(login).await;
    assert!(
        !body["csrf_token"].as_str().unwrap().is_empty(),
        "login hands out the CSRF token"
    );

    let response = actix_test::call_service(
        &app,
        actix_test::TestRequest::get()
            .uri("/probe/whoami")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let identity: serde_json::Value = actix_test::read_body_json(response).await;
    assert_eq!(identity["user_id"], serde_json::json!(user_id));
    assert_eq!(
        identity["institution_id"],
        serde_json::json!(institution_id)
    );
    assert_eq!(identity["student_id"], serde_json::json!(student_id));
    assert_eq!(identity["roles"], serde_json::json!(["student"]));
}

#[sqlx::test(migrations = "./migrations")]
async fn login_failures_all_get_the_same_generic_401(pool: PgPool) {
    let institution_id = seed_institution(&pool).await;
    seed_credentialed_user(
        &pool,
        institution_id,
        "known.user",
        "right-password",
        "active",
    )
    .await;
    seed_credentialed_user(
        &pool,
        institution_id,
        "sus.user",
        "any-password",
        "suspended",
    )
    .await;

    let app = test_app!(&pool, institution_id);

    let mut bodies = Vec::new();
    for (username, password) in [
        ("known.user", "wrong-password"),
        ("no.such.user", "any-password"),
        ("sus.user", "any-password"),
    ] {
        let response =
            actix_test::call_service(&app, login_request(username, password).to_request()).await;
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "case {username}"
        );
        bodies.push(actix_test::read_body(response).await);
    }

    assert_eq!(bodies[0], bodies[1], "unknown user is indistinguishable");
    assert_eq!(bodies[0], bodies[2], "suspension is indistinguishable");
}

#[sqlx::test(migrations = "./migrations")]
async fn full_session_lifecycle_login_use_logout(pool: PgPool) {
    let institution_id = seed_institution(&pool).await;
    seed_credentialed_user(&pool, institution_id, "cycle.user", "pw-cycle-1", "active").await;
    let app = test_app!(&pool, institution_id);

    let (cookie, csrf) = login_session!(&app, "cycle.user", "pw-cycle-1");

    let authed = actix_test::call_service(
        &app,
        actix_test::TestRequest::get()
            .uri("/probe/whoami")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(authed.status(), StatusCode::OK);

    let logout = actix_test::call_service(
        &app,
        actix_test::TestRequest::post()
            .uri("/api/v1/session/logout")
            .cookie(cookie.clone())
            .insert_header((crate::identity_access::csrf::CSRF_HEADER, csrf.as_str()))
            .to_request(),
    )
    .await;
    assert_eq!(logout.status(), StatusCode::NO_CONTENT);
    let removal = session_cookie_from(&logout);
    assert_eq!(removal.value(), "", "logout clears the cookie");

    let after = actix_test::call_service(
        &app,
        actix_test::TestRequest::get()
            .uri("/probe/whoami")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(after.status(), StatusCode::UNAUTHORIZED);

    // Logging out again with the dead cookie: no session resolves, so 401.
    let double_logout = actix_test::call_service(
        &app,
        actix_test::TestRequest::post()
            .uri("/api/v1/session/logout")
            .to_request(),
    )
    .await;
    assert_eq!(double_logout.status(), StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "./migrations")]
async fn expired_sessions_stop_working_over_http(pool: PgPool) {
    let institution_id = seed_institution(&pool).await;
    let user_id =
        seed_credentialed_user(&pool, institution_id, "sleepy.user", "pw-sleepy", "active").await;
    let app = test_app!(&pool, institution_id);

    let login =
        actix_test::call_service(&app, login_request("sleepy.user", "pw-sleepy").to_request())
            .await;
    let cookie = session_cookie_from(&login);

    sqlx::query(
        "UPDATE user_session SET idle_expires_at = now() - interval '1 second' \
         WHERE user_id = $1",
    )
    .bind(user_id)
    .execute(&pool)
    .await
    .unwrap();

    let response = actix_test::call_service(
        &app,
        actix_test::TestRequest::get()
            .uri("/probe/whoami")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "./migrations")]
async fn logging_in_again_rotates_the_presented_session(pool: PgPool) {
    let institution_id = seed_institution(&pool).await;
    seed_credentialed_user(&pool, institution_id, "rotate.user", "pw-rotate", "active").await;
    let app = test_app!(&pool, institution_id);

    let first =
        actix_test::call_service(&app, login_request("rotate.user", "pw-rotate").to_request())
            .await;
    let first_cookie = session_cookie_from(&first);

    // Second login presents the first session's cookie; the server must
    // rotate: revoke the old session and issue a fresh token.
    let second = actix_test::call_service(
        &app,
        login_request("rotate.user", "pw-rotate")
            .cookie(first_cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(second.status(), StatusCode::OK);
    let second_cookie = session_cookie_from(&second);
    assert_ne!(first_cookie.value(), second_cookie.value());

    let with_old = actix_test::call_service(
        &app,
        actix_test::TestRequest::get()
            .uri("/probe/whoami")
            .cookie(first_cookie)
            .to_request(),
    )
    .await;
    assert_eq!(with_old.status(), StatusCode::UNAUTHORIZED);

    let with_new = actix_test::call_service(
        &app,
        actix_test::TestRequest::get()
            .uri("/probe/whoami")
            .cookie(second_cookie)
            .to_request(),
    )
    .await;
    assert_eq!(with_new.status(), StatusCode::OK);
}

#[sqlx::test(migrations = "./migrations")]
async fn throttle_locks_an_account_ip_pair_then_expires(pool: PgPool) {
    let institution_id = seed_institution(&pool).await;
    seed_credentialed_user(&pool, institution_id, "target.user", "pw-target", "active").await;
    let auth = auth_svc(&pool);

    for _ in 0..TEST_MAX_FAILURES {
        let err = auth
            .login(institution_id, "target.user", "wrong", "203.0.113.7")
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            crate::shared::error::AppError::Unauthenticated
        ));
    }

    // Budget exhausted: even the CORRECT password is rejected with 429 from
    // this address...
    let throttled = auth
        .login(institution_id, "target.user", "pw-target", "203.0.113.7")
        .await
        .unwrap_err();
    assert!(matches!(
        throttled,
        crate::shared::error::AppError::RateLimited
    ));

    // ...while the account stays usable from another address (per
    // account+IP, so an attacker cannot lock the real user out globally).
    assert!(
        auth.login(institution_id, "target.user", "pw-target", "198.51.100.9")
            .await
            .is_ok()
    );

    // The window is fixed, not ever-growing: once it lapses, the pair
    // recovers on its own.
    sqlx::query(
        "UPDATE login_throttle SET window_started_at = now() - interval '16 minutes' \
         WHERE client_ip = '203.0.113.7'",
    )
    .execute(&pool)
    .await
    .unwrap();

    assert!(
        auth.login(institution_id, "target.user", "pw-target", "203.0.113.7")
            .await
            .is_ok()
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn successful_login_resets_the_failure_budget(pool: PgPool) {
    let institution_id = seed_institution(&pool).await;
    seed_credentialed_user(&pool, institution_id, "reset.user", "pw-reset", "active").await;
    let auth = auth_svc(&pool);

    for _ in 0..(TEST_MAX_FAILURES - 1) {
        let _ = auth
            .login(institution_id, "reset.user", "wrong", "203.0.113.7")
            .await;
    }
    assert!(
        auth.login(institution_id, "reset.user", "pw-reset", "203.0.113.7")
            .await
            .is_ok()
    );

    // The success cleared the counter; the same pair has its full budget back.
    let remaining: Option<i32> = sqlx::query_scalar(
        "SELECT failure_count FROM login_throttle WHERE client_ip = '203.0.113.7'",
    )
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert_eq!(remaining, None);
}

#[sqlx::test(migrations = "./migrations")]
async fn csrf_missing_wrong_and_cross_session_tokens_are_403(pool: PgPool) {
    let institution_id = seed_institution(&pool).await;
    seed_credentialed_user(&pool, institution_id, "alpha.user", "pw-alpha", "active").await;
    seed_credentialed_user(&pool, institution_id, "bravo.user", "pw-bravo", "active").await;
    let app = test_app!(&pool, institution_id);

    let (cookie_a, csrf_a) = login_session!(&app, "alpha.user", "pw-alpha");
    let (_cookie_b, csrf_b) = login_session!(&app, "bravo.user", "pw-bravo");

    // Missing token.
    let missing = actix_test::call_service(
        &app,
        actix_test::TestRequest::post()
            .uri("/api/v1/session/logout")
            .cookie(cookie_a.clone())
            .to_request(),
    )
    .await;
    assert_eq!(missing.status(), StatusCode::FORBIDDEN);

    // Wrong token.
    let wrong = actix_test::call_service(
        &app,
        actix_test::TestRequest::post()
            .uri("/api/v1/session/logout")
            .cookie(cookie_a.clone())
            .insert_header((crate::identity_access::csrf::CSRF_HEADER, "deadbeef"))
            .to_request(),
    )
    .await;
    assert_eq!(wrong.status(), StatusCode::FORBIDDEN);

    // A perfectly valid token — minted for a DIFFERENT session.
    let cross = actix_test::call_service(
        &app,
        actix_test::TestRequest::post()
            .uri("/api/v1/session/logout")
            .cookie(cookie_a.clone())
            .insert_header((crate::identity_access::csrf::CSRF_HEADER, csrf_b.as_str()))
            .to_request(),
    )
    .await;
    assert_eq!(cross.status(), StatusCode::FORBIDDEN);

    // None of those rejections killed the session...
    let alive = actix_test::call_service(
        &app,
        actix_test::TestRequest::get()
            .uri("/probe/whoami")
            .cookie(cookie_a.clone())
            .to_request(),
    )
    .await;
    assert_eq!(alive.status(), StatusCode::OK);

    // ...and the right token succeeds.
    let success = actix_test::call_service(
        &app,
        actix_test::TestRequest::post()
            .uri("/api/v1/session/logout")
            .cookie(cookie_a)
            .insert_header((crate::identity_access::csrf::CSRF_HEADER, csrf_a.as_str()))
            .to_request(),
    )
    .await;
    assert_eq!(success.status(), StatusCode::NO_CONTENT);
}

#[sqlx::test(migrations = "./migrations")]
async fn csrf_form_field_is_accepted_and_the_body_survives_for_the_handler(pool: PgPool) {
    let institution_id = seed_institution(&pool).await;
    seed_credentialed_user(&pool, institution_id, "form.user", "pw-form", "active").await;
    let app = test_app!(&pool, institution_id);

    let (cookie, csrf) = login_session!(&app, "form.user", "pw-form");

    let ok = actix_test::call_service(
        &app,
        actix_test::TestRequest::post()
            .uri("/probe/form")
            .cookie(cookie.clone())
            .insert_header((
                actix_web::http::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            ))
            .set_payload(format!("note=hello&csrf_token={csrf}"))
            .to_request(),
    )
    .await;
    assert_eq!(ok.status(), StatusCode::OK);
    let body = actix_test::read_body(ok).await;
    assert_eq!(
        body, "noted:hello",
        "handler still received the buffered form body"
    );

    let bad = actix_test::call_service(
        &app,
        actix_test::TestRequest::post()
            .uri("/probe/form")
            .cookie(cookie)
            .insert_header((
                actix_web::http::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            ))
            .set_payload("note=hello&csrf_token=forged")
            .to_request(),
    )
    .await;
    assert_eq!(bad.status(), StatusCode::FORBIDDEN);
}

// ---- Rotation and revocation triggers (slice 2.6) ----

#[sqlx::test(migrations = "./migrations")]
async fn password_change_rotates_this_session_and_revokes_all_others(pool: PgPool) {
    let institution_id = seed_institution(&pool).await;
    let user_id = seed_credentialed_user(
        &pool,
        institution_id,
        "chg.user",
        "old-password-12",
        "active",
    )
    .await;
    let app = test_app!(&pool, institution_id);

    // Two live sessions: this browser and, say, a phone.
    let (cookie_here, csrf_here) = login_session!(&app, "chg.user", "old-password-12");
    let (cookie_phone, _) = login_session!(&app, "chg.user", "old-password-12");

    let change = actix_test::call_service(
        &app,
        actix_test::TestRequest::post()
            .uri("/api/v1/me/password")
            .cookie(cookie_here.clone())
            .insert_header((
                crate::identity_access::csrf::CSRF_HEADER,
                csrf_here.as_str(),
            ))
            .set_json(serde_json::json!({
                "current_password": "old-password-12",
                "new_password": "brand-new-password-1",
            }))
            .to_request(),
    )
    .await;
    assert_eq!(change.status(), StatusCode::OK);

    // Rotation: a NEW cookie for this client, with a new CSRF token.
    let fresh_cookie = session_cookie_from(&change);
    assert_ne!(fresh_cookie.value(), cookie_here.value());
    let body: serde_json::Value = actix_test::read_body_json(change).await;
    assert!(!body["csrf_token"].as_str().unwrap().is_empty());

    // Revocation: BOTH old sessions are dead; the fresh one works.
    for (name, dead) in [("here", &cookie_here), ("phone", &cookie_phone)] {
        let response = actix_test::call_service(
            &app,
            actix_test::TestRequest::get()
                .uri("/probe/whoami")
                .cookie(dead.clone())
                .to_request(),
        )
        .await;
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "old {name} session"
        );
    }
    let alive = actix_test::call_service(
        &app,
        actix_test::TestRequest::get()
            .uri("/probe/whoami")
            .cookie(fresh_cookie)
            .to_request(),
    )
    .await;
    assert_eq!(alive.status(), StatusCode::OK);

    // The old password is dead; the new one works.
    let old_pw = actix_test::call_service(
        &app,
        login_request("chg.user", "old-password-12").to_request(),
    )
    .await;
    assert_eq!(old_pw.status(), StatusCode::UNAUTHORIZED);
    let _ = login_session!(&app, "chg.user", "brand-new-password-1");

    // Audited in the same transaction as the change, actor = the user.
    let audited: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_event WHERE institution_id = $1 \
         AND action = 'identity.password_changed' AND actor_user_id = $2",
    )
    .bind(institution_id)
    .bind(user_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(audited, 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn password_change_rejects_wrong_current_and_short_new(pool: PgPool) {
    let institution_id = seed_institution(&pool).await;
    seed_credentialed_user(
        &pool,
        institution_id,
        "keep.user",
        "keeper-password-1",
        "active",
    )
    .await;
    let app = test_app!(&pool, institution_id);

    let (cookie, csrf) = login_session!(&app, "keep.user", "keeper-password-1");

    // Wrong current password: rejected, nothing rotates.
    let wrong_current = actix_test::call_service(
        &app,
        actix_test::TestRequest::post()
            .uri("/api/v1/me/password")
            .cookie(cookie.clone())
            .insert_header((crate::identity_access::csrf::CSRF_HEADER, csrf.as_str()))
            .set_json(serde_json::json!({
                "current_password": "not-my-password",
                "new_password": "brand-new-password-1",
            }))
            .to_request(),
    )
    .await;
    assert_eq!(wrong_current.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // Too-short replacement: rejected even with the right current password.
    let short_new = actix_test::call_service(
        &app,
        actix_test::TestRequest::post()
            .uri("/api/v1/me/password")
            .cookie(cookie.clone())
            .insert_header((crate::identity_access::csrf::CSRF_HEADER, csrf.as_str()))
            .set_json(serde_json::json!({
                "current_password": "keeper-password-1",
                "new_password": "short",
            }))
            .to_request(),
    )
    .await;
    assert_eq!(short_new.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // Neither attempt killed the session or the password.
    let still_alive = actix_test::call_service(
        &app,
        actix_test::TestRequest::get()
            .uri("/probe/whoami")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(still_alive.status(), StatusCode::OK);
    let _ = login_session!(&app, "keep.user", "keeper-password-1");
}

#[sqlx::test(migrations = "./migrations")]
async fn admin_password_reset_revokes_target_sessions(pool: PgPool) {
    let institution_id = seed_institution(&pool).await;
    let admin_id = seed_credentialed_user(
        &pool,
        institution_id,
        "the.admin",
        "admin-password-1",
        "active",
    )
    .await;
    assign_role(&pool, institution_id, admin_id, "institution_admin").await;
    let target_id = seed_credentialed_user(
        &pool,
        institution_id,
        "plain.user",
        "user-password-1",
        "active",
    )
    .await;
    let app = test_app!(&pool, institution_id);

    let (admin_cookie, admin_csrf) = login_session!(&app, "the.admin", "admin-password-1");
    let (target_cookie, target_csrf) = login_session!(&app, "plain.user", "user-password-1");

    // A non-admin cannot reset anyone's password — not even their own via
    // this route.
    let forbidden = actix_test::call_service(
        &app,
        actix_test::TestRequest::post()
            .uri(&format!("/api/v1/users/{admin_id}/password"))
            .cookie(target_cookie.clone())
            .insert_header((
                crate::identity_access::csrf::CSRF_HEADER,
                target_csrf.as_str(),
            ))
            .set_json(serde_json::json!({ "new_password": "hijacked-password-1" }))
            .to_request(),
    )
    .await;
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);

    // The admin resets the target's password.
    let reset = actix_test::call_service(
        &app,
        actix_test::TestRequest::post()
            .uri(&format!("/api/v1/users/{target_id}/password"))
            .cookie(admin_cookie.clone())
            .insert_header((
                crate::identity_access::csrf::CSRF_HEADER,
                admin_csrf.as_str(),
            ))
            .set_json(serde_json::json!({ "new_password": "issued-password-99" }))
            .to_request(),
    )
    .await;
    assert_eq!(reset.status(), StatusCode::NO_CONTENT);

    // The target's session is gone; the old password is dead; the new works.
    let dead = actix_test::call_service(
        &app,
        actix_test::TestRequest::get()
            .uri("/probe/whoami")
            .cookie(target_cookie)
            .to_request(),
    )
    .await;
    assert_eq!(dead.status(), StatusCode::UNAUTHORIZED);
    let old_pw = actix_test::call_service(
        &app,
        login_request("plain.user", "user-password-1").to_request(),
    )
    .await;
    assert_eq!(old_pw.status(), StatusCode::UNAUTHORIZED);
    let _ = login_session!(&app, "plain.user", "issued-password-99");

    // The admin's own session was untouched, and the reset was audited with
    // the ADMIN as the actor.
    let admin_alive = actix_test::call_service(
        &app,
        actix_test::TestRequest::get()
            .uri("/probe/whoami")
            .cookie(admin_cookie)
            .to_request(),
    )
    .await;
    assert_eq!(admin_alive.status(), StatusCode::OK);
    let audited: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_event WHERE institution_id = $1 \
         AND action = 'identity.password_reset' AND actor_user_id = $2",
    )
    .bind(institution_id)
    .bind(admin_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(audited, 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn suspension_revokes_sessions_and_blocks_relogin(pool: PgPool) {
    let institution_id = seed_institution(&pool).await;
    let admin_id = seed_credentialed_user(
        &pool,
        institution_id,
        "sus.admin",
        "admin-password-1",
        "active",
    )
    .await;
    assign_role(&pool, institution_id, admin_id, "institution_admin").await;
    let target_id = seed_credentialed_user(
        &pool,
        institution_id,
        "doomed.user",
        "user-password-1",
        "active",
    )
    .await;
    let app = test_app!(&pool, institution_id);

    let (admin_cookie, admin_csrf) = login_session!(&app, "sus.admin", "admin-password-1");
    let (target_cookie, _) = login_session!(&app, "doomed.user", "user-password-1");

    // A blank reason is not a reason.
    let no_reason = actix_test::call_service(
        &app,
        actix_test::TestRequest::post()
            .uri(&format!("/api/v1/users/{target_id}/suspend"))
            .cookie(admin_cookie.clone())
            .insert_header((
                crate::identity_access::csrf::CSRF_HEADER,
                admin_csrf.as_str(),
            ))
            .set_json(serde_json::json!({ "reason": "   " }))
            .to_request(),
    )
    .await;
    assert_eq!(no_reason.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // Self-suspension is refused: the last admin cannot brick the deployment.
    let self_suspend = actix_test::call_service(
        &app,
        actix_test::TestRequest::post()
            .uri(&format!("/api/v1/users/{admin_id}/suspend"))
            .cookie(admin_cookie.clone())
            .insert_header((
                crate::identity_access::csrf::CSRF_HEADER,
                admin_csrf.as_str(),
            ))
            .set_json(serde_json::json!({ "reason": "testing" }))
            .to_request(),
    )
    .await;
    assert_eq!(self_suspend.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // The real suspension.
    let suspend = actix_test::call_service(
        &app,
        actix_test::TestRequest::post()
            .uri(&format!("/api/v1/users/{target_id}/suspend"))
            .cookie(admin_cookie.clone())
            .insert_header((
                crate::identity_access::csrf::CSRF_HEADER,
                admin_csrf.as_str(),
            ))
            .set_json(serde_json::json!({ "reason": "academic integrity case 44" }))
            .to_request(),
    )
    .await;
    assert_eq!(suspend.status(), StatusCode::NO_CONTENT);

    // Session revoked, and the CORRECT password no longer signs in — with the
    // same generic 401 an attacker would see for a wrong one.
    let dead = actix_test::call_service(
        &app,
        actix_test::TestRequest::get()
            .uri("/probe/whoami")
            .cookie(target_cookie)
            .to_request(),
    )
    .await;
    assert_eq!(dead.status(), StatusCode::UNAUTHORIZED);
    let relogin = actix_test::call_service(
        &app,
        login_request("doomed.user", "user-password-1").to_request(),
    )
    .await;
    assert_eq!(relogin.status(), StatusCode::UNAUTHORIZED);

    let audited: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_event WHERE institution_id = $1 \
         AND action = 'identity.user_suspended' AND actor_user_id = $2",
    )
    .bind(institution_id)
    .bind(admin_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(audited, 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn admin_powers_stop_at_the_institution_boundary(pool: PgPool) {
    let institution_a = seed_institution(&pool).await;
    let institution_b = seed_institution(&pool).await;
    let admin_id = seed_credentialed_user(
        &pool,
        institution_a,
        "a.admin",
        "admin-password-1",
        "active",
    )
    .await;
    assign_role(&pool, institution_a, admin_id, "institution_admin").await;
    let outsider_id =
        seed_credentialed_user(&pool, institution_b, "b.user", "user-password-1", "active").await;
    let app = test_app!(&pool, institution_a);

    let (admin_cookie, admin_csrf) = login_session!(&app, "a.admin", "admin-password-1");

    // Cross-institution reset and suspension both answer 404 — the other
    // institution's accounts do not exist as far as this admin can tell.
    let reset = actix_test::call_service(
        &app,
        actix_test::TestRequest::post()
            .uri(&format!("/api/v1/users/{outsider_id}/password"))
            .cookie(admin_cookie.clone())
            .insert_header((
                crate::identity_access::csrf::CSRF_HEADER,
                admin_csrf.as_str(),
            ))
            .set_json(serde_json::json!({ "new_password": "stolen-password-1" }))
            .to_request(),
    )
    .await;
    assert_eq!(reset.status(), StatusCode::NOT_FOUND);

    let suspend = actix_test::call_service(
        &app,
        actix_test::TestRequest::post()
            .uri(&format!("/api/v1/users/{outsider_id}/suspend"))
            .cookie(admin_cookie)
            .insert_header((
                crate::identity_access::csrf::CSRF_HEADER,
                admin_csrf.as_str(),
            ))
            .set_json(serde_json::json!({ "reason": "reaching across the fence" }))
            .to_request(),
    )
    .await;
    assert_eq!(suspend.status(), StatusCode::NOT_FOUND);

    // The outsider is untouched: still active, sessions intact, no audit
    // trail was written against institution B.
    let status: String = sqlx::query_scalar("SELECT status::text FROM user_account WHERE id = $1")
        .bind(outsider_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "active");
    let b_audit: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_event WHERE institution_id = $1")
            .bind(institution_b)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(b_audit, 0);
}

// ---- Role assignment (slice 2.7) ----

#[sqlx::test(migrations = "./migrations")]
async fn admin_grants_and_revokes_roles_with_session_revocation(pool: PgPool) {
    let institution_id = seed_institution(&pool).await;
    let admin_id = seed_credentialed_user(
        &pool,
        institution_id,
        "role.admin",
        "admin-password-1",
        "active",
    )
    .await;
    assign_role(&pool, institution_id, admin_id, "institution_admin").await;
    let target_id = seed_credentialed_user(
        &pool,
        institution_id,
        "future.registrar",
        "user-password-1",
        "active",
    )
    .await;
    let app = test_app!(&pool, institution_id);

    let (admin_cookie, admin_csrf) = login_session!(&app, "role.admin", "admin-password-1");
    let (target_cookie, _) = login_session!(&app, "future.registrar", "user-password-1");

    // Grant: 204, and the privilege change revokes the target's sessions.
    let grant = actix_test::call_service(
        &app,
        actix_test::TestRequest::post()
            .uri(&format!("/api/v1/users/{target_id}/roles"))
            .cookie(admin_cookie.clone())
            .insert_header((
                crate::identity_access::csrf::CSRF_HEADER,
                admin_csrf.as_str(),
            ))
            .set_json(serde_json::json!({ "role": "registrar" }))
            .to_request(),
    )
    .await;
    assert_eq!(grant.status(), StatusCode::NO_CONTENT);

    let dead = actix_test::call_service(
        &app,
        actix_test::TestRequest::get()
            .uri("/probe/whoami")
            .cookie(target_cookie)
            .to_request(),
    )
    .await;
    assert_eq!(dead.status(), StatusCode::UNAUTHORIZED);

    // A fresh session carries the new role.
    let (target_cookie, _) = login_session!(&app, "future.registrar", "user-password-1");
    let identity = actix_test::call_service(
        &app,
        actix_test::TestRequest::get()
            .uri("/probe/whoami")
            .cookie(target_cookie.clone())
            .to_request(),
    )
    .await;
    let body: serde_json::Value = actix_test::read_body_json(identity).await;
    assert_eq!(body["roles"], serde_json::json!(["registrar"]));

    // Granting the same role again is idempotent: no second audit record,
    // and — crucially for retried requests — no session revocation.
    let regrant = actix_test::call_service(
        &app,
        actix_test::TestRequest::post()
            .uri(&format!("/api/v1/users/{target_id}/roles"))
            .cookie(admin_cookie.clone())
            .insert_header((
                crate::identity_access::csrf::CSRF_HEADER,
                admin_csrf.as_str(),
            ))
            .set_json(serde_json::json!({ "role": "registrar" }))
            .to_request(),
    )
    .await;
    assert_eq!(regrant.status(), StatusCode::NO_CONTENT);
    let survived = actix_test::call_service(
        &app,
        actix_test::TestRequest::get()
            .uri("/probe/whoami")
            .cookie(target_cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(
        survived.status(),
        StatusCode::OK,
        "idempotent regrant must not revoke sessions"
    );
    let granted_audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_event \
         WHERE action = 'identity.role_granted' AND resource_id = $1",
    )
    .bind(target_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(granted_audits, 1);

    // Revoke: 204, sessions die again, and the next session has no roles.
    let revoke = actix_test::call_service(
        &app,
        actix_test::TestRequest::delete()
            .uri(&format!("/api/v1/users/{target_id}/roles/registrar"))
            .cookie(admin_cookie.clone())
            .insert_header((
                crate::identity_access::csrf::CSRF_HEADER,
                admin_csrf.as_str(),
            ))
            .to_request(),
    )
    .await;
    assert_eq!(revoke.status(), StatusCode::NO_CONTENT);

    let dead_again = actix_test::call_service(
        &app,
        actix_test::TestRequest::get()
            .uri("/probe/whoami")
            .cookie(target_cookie)
            .to_request(),
    )
    .await;
    assert_eq!(dead_again.status(), StatusCode::UNAUTHORIZED);

    let (target_cookie, _) = login_session!(&app, "future.registrar", "user-password-1");
    let identity = actix_test::call_service(
        &app,
        actix_test::TestRequest::get()
            .uri("/probe/whoami")
            .cookie(target_cookie)
            .to_request(),
    )
    .await;
    let body: serde_json::Value = actix_test::read_body_json(identity).await;
    assert_eq!(body["roles"], serde_json::json!([]));

    let revoked_audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_event \
         WHERE action = 'identity.role_revoked' AND resource_id = $1",
    )
    .bind(target_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(revoked_audits, 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn role_management_guardrails(pool: PgPool) {
    let institution_a = seed_institution(&pool).await;
    let institution_b = seed_institution(&pool).await;
    let admin_id = seed_credentialed_user(
        &pool,
        institution_a,
        "guard.admin",
        "admin-password-1",
        "active",
    )
    .await;
    assign_role(&pool, institution_a, admin_id, "institution_admin").await;
    let target_id = seed_credentialed_user(
        &pool,
        institution_a,
        "guard.user",
        "user-password-1",
        "active",
    )
    .await;
    let outsider_id = seed_credentialed_user(
        &pool,
        institution_b,
        "guard.outsider",
        "user-password-1",
        "active",
    )
    .await;
    let app = test_app!(&pool, institution_a);

    let (admin_cookie, admin_csrf) = login_session!(&app, "guard.admin", "admin-password-1");
    let (user_cookie, user_csrf) = login_session!(&app, "guard.user", "user-password-1");

    let grant =
        |cookie: &actix_web::cookie::Cookie<'static>, csrf: &str, user: Uuid, role: &str| {
            actix_test::TestRequest::post()
                .uri(&format!("/api/v1/users/{user}/roles"))
                .cookie(cookie.clone())
                .insert_header((crate::identity_access::csrf::CSRF_HEADER, csrf.to_owned()))
                .set_json(serde_json::json!({ "role": role }))
                .to_request()
        };

    // A non-admin cannot grant — not even to themselves.
    let forbidden = actix_test::call_service(
        &app,
        grant(&user_cookie, &user_csrf, target_id, "registrar"),
    )
    .await;
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);

    // The platform licensing role is not grantable through this API at all.
    let platform = actix_test::call_service(
        &app,
        grant(
            &admin_cookie,
            &admin_csrf,
            target_id,
            "platform_licensing_admin",
        ),
    )
    .await;
    assert_eq!(platform.status(), StatusCode::FORBIDDEN);

    // ...nor revocable: an institution admin cannot strip the platform
    // operator's access.
    let strip_platform = actix_test::call_service(
        &app,
        actix_test::TestRequest::delete()
            .uri(&format!(
                "/api/v1/users/{target_id}/roles/platform_licensing_admin"
            ))
            .cookie(admin_cookie.clone())
            .insert_header((
                crate::identity_access::csrf::CSRF_HEADER,
                admin_csrf.as_str(),
            ))
            .to_request(),
    )
    .await;
    assert_eq!(strip_platform.status(), StatusCode::FORBIDDEN);

    // Admins cannot edit their own privileges.
    let self_grant = actix_test::call_service(
        &app,
        grant(&admin_cookie, &admin_csrf, admin_id, "registrar"),
    )
    .await;
    assert_eq!(self_grant.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // Unknown role codes are a validation error, not a 500.
    let unknown = actix_test::call_service(
        &app,
        grant(&admin_cookie, &admin_csrf, target_id, "super_admin"),
    )
    .await;
    assert_eq!(unknown.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // Another institution's account looks like a missing user.
    let cross = actix_test::call_service(
        &app,
        grant(&admin_cookie, &admin_csrf, outsider_id, "registrar"),
    )
    .await;
    assert_eq!(cross.status(), StatusCode::NOT_FOUND);

    // None of the failures granted anything or wrote an audit record.
    let role_rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM user_role WHERE user_id IN ($1, $2, $3) \
         AND role_id <> (SELECT id FROM role WHERE code = 'institution_admin')",
    )
    .bind(admin_id)
    .bind(target_id)
    .bind(outsider_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(role_rows, 0);
    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_event \
         WHERE action IN ('identity.role_granted', 'identity.role_revoked')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(audits, 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn signed_out_browsers_land_on_sign_in_but_api_clients_keep_401(pool: PgPool) {
    let institution_id = seed_institution(&pool).await;
    let app = test_app!(&pool, institution_id);

    // A protected UI page without a session: redirect to the sign-in page.
    let response = actix_test::call_service(
        &app,
        actix_test::TestRequest::get()
            .uri("/ui/signout")
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response.headers().get("location").unwrap(),
        "/ui/login",
        "browser navigation must land on sign-in"
    );

    // An API route stays an honest 401 for programmatic clients.
    let response = actix_test::call_service(
        &app,
        actix_test::TestRequest::get()
            .uri("/probe/whoami")
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // The sign-in page itself never redirects to itself.
    let response = actix_test::call_service(
        &app,
        actix_test::TestRequest::get().uri("/ui/login").to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[sqlx::test(migrations = "./migrations")]
async fn ui_signout_revokes_the_session_and_clears_the_cookie(pool: PgPool) {
    let institution_id = seed_institution(&pool).await;
    seed_credentialed_user(
        &pool,
        institution_id,
        "signout.user",
        "pw-signout",
        "active",
    )
    .await;
    let app = test_app!(&pool, institution_id);

    let (cookie, csrf) = login_session!(&app, "signout.user", "pw-signout");

    // The confirmation page renders a real CSRF-protected form.
    let response = actix_test::call_service(
        &app,
        actix_test::TestRequest::get()
            .uri("/ui/signout")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = String::from_utf8(actix_test::read_body(response).await.to_vec()).unwrap();
    assert!(body.contains("form method=\"post\" action=\"/ui/signout\""));
    assert!(body.contains(&csrf));
    crate::shared::assets::assert_page_a11y(&body);

    // Signing out without the token is refused (cross-site POST).
    let forged = actix_test::call_service(
        &app,
        actix_test::TestRequest::post()
            .uri("/ui/signout")
            .cookie(cookie.clone())
            .insert_header((
                actix_web::http::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            ))
            .set_payload("csrf_token=forged")
            .to_request(),
    )
    .await;
    assert_eq!(forged.status(), StatusCode::FORBIDDEN);

    // With the token: session revoked, cookie removed, back to sign-in.
    let response = actix_test::call_service(
        &app,
        actix_test::TestRequest::post()
            .uri("/ui/signout")
            .cookie(cookie.clone())
            .insert_header((
                actix_web::http::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            ))
            .set_payload(format!("csrf_token={csrf}"))
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers().get("location").unwrap(), "/ui/login");
    let removal = session_cookie_from(&response);
    assert_eq!(removal.value(), "", "cookie must be cleared");

    // The old cookie is dead server-side: the browser lands on sign-in.
    let response = actix_test::call_service(
        &app,
        actix_test::TestRequest::get()
            .uri("/ui/signout")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers().get("location").unwrap(), "/ui/login");
}
