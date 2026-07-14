// mod academics;
mod app;
mod audit;
mod config;
mod db;
mod documents;
mod enrollment;
mod identity_access;
// mod institution;
// mod jobs;
mod licensing;
mod records;
mod shared;

use crate::audit::AuditWriter;
use crate::config::AppConfig;
use crate::documents::{DocumentService, DocumentWorker};
use crate::enrollment::EnrollmentService;
use crate::identity_access::http::SessionCookiePolicy;
use crate::identity_access::password::PasswordService;
use crate::identity_access::service::AuthService;
use crate::identity_access::sessions::SessionService;
use crate::licensing::{LicenseGate, LicenseService, LicenseSnapshot, LicenseStatus};
use crate::records::{GradeService, ScheduleQuery, TranscriptSnapshotService};
use crate::shared::error::AppError;
use actix_web::{App, HttpServer, middleware, web};

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    // Development convenience only: values from .env never override real
    // environment variables, so production configuration always wins.
    dotenvy::dotenv().ok();

    crate::shared::observability::init_tracing();

    let config = AppConfig::from_env().unwrap_or_else(|error| {
        // Startup abort is the correct response to bad configuration; the
        // message never contains configured values (see ConfigError).
        eprintln!("configuration error: {error}");
        std::process::exit(1);
    });

    let pool = db::connect_and_migrate(&config)
        .await
        .expect("database connection or migration failed");

    let audit = AuditWriter;
    let initial_license = load_initial_license(&pool)
        .await
        .expect("valid institution license required before startup");
    let license_gate = LicenseGate::new(initial_license);

    let passwords =
        PasswordService::from_config(&config).expect("valid Argon2 parameters required");
    let sessions = SessionService::new(
        pool.clone(),
        config.session_idle_secs,
        config.session_absolute_secs,
    );
    let auth = AuthService::new(
        pool.clone(),
        passwords,
        sessions.clone(),
        audit.clone(),
        config.login_max_failures,
        config.login_throttle_window_secs,
    )
    .expect("auth service initialization failed");
    let cookie_policy = SessionCookiePolicy {
        secure: config.environment.is_production(),
        max_age_secs: config.session_absolute_secs as i64,
    };

    let enrollment = EnrollmentService::new(pool.clone(), audit.clone());
    let grades = GradeService::new(pool.clone(), audit.clone());
    let schedule = ScheduleQuery::new(pool.clone());
    let transcript = TranscriptSnapshotService;
    let documents = DocumentService::new(pool.clone(), audit.clone(), transcript);
    let licensing = LicenseService::new(pool.clone(), license_gate.clone(), audit);

    let worker = DocumentWorker::new(
        pool.clone(),
        config.worker_id.clone(),
        config.document_storage_path.clone(),
    );
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let worker_handle = actix_web::rt::spawn(worker.run(shutdown_rx));

    // Startup already proved the database works (migrations, license load),
    // so the flag starts true; the prober keeps it honest from here on.
    let readiness = app::Readiness::new(true);
    app::spawn_readiness_prober(
        pool.clone(),
        readiness.clone(),
        config.readiness_interval_secs,
    );

    let environment = config.environment;
    let server = HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(pool.clone()))
            .app_data(web::Data::new(enrollment.clone()))
            .app_data(web::Data::new(grades.clone()))
            .app_data(web::Data::new(schedule.clone()))
            .app_data(web::Data::new(documents.clone()))
            .app_data(web::Data::new(licensing.clone()))
            .app_data(web::Data::new(license_gate.clone()))
            .app_data(web::Data::new(readiness.clone()))
            .app_data(web::Data::new(sessions.clone()))
            .app_data(web::Data::new(auth.clone()))
            .app_data(web::Data::new(cookie_policy))
            // Bounded request bodies; raise per-route when a real need appears.
            .app_data(web::JsonConfig::default().limit(64 * 1024))
            .app_data(web::FormConfig::default().limit(64 * 1024))
            .app_data(web::PayloadConfig::new(256 * 1024))
            // Registered first = runs innermost: CSRF checks need the
            // session already resolved, so csrf sits inside session.
            .wrap(middleware::from_fn(
                crate::identity_access::csrf::csrf_middleware,
            ))
            .wrap(middleware::from_fn(
                crate::identity_access::middleware::session_middleware,
            ))
            // License gate sits outside the session: a locked deployment
            // answers 402 before any database work happens.
            .wrap(middleware::from_fn(
                crate::licensing::middleware::license_middleware,
            ))
            .wrap(middleware::NormalizePath::trim())
            .wrap(app::security_headers(environment))
            // Correlation + completion logging with redaction by construction;
            // replaces Logger::default(), which would log full query strings.
            .wrap(middleware::from_fn(
                crate::shared::observability::request_id_middleware,
            ))
            .configure(crate::app::recovery_routes)
            .configure(crate::app::protected_routes)
    })
    .workers(std::thread::available_parallelism().map_or(1, usize::from))
    .shutdown_timeout(config.shutdown_timeout_secs)
    .bind(config.bind_addr)?
    .run();

    // Actix stops accepting and drains handlers on SIGINT/SIGTERM; after
    // that, tell the worker to finish its current job and wait for it.
    let result = server.await;

    let _ = shutdown_tx.send(true);
    if tokio::time::timeout(
        std::time::Duration::from_secs(config.shutdown_timeout_secs),
        worker_handle,
    )
    .await
    .is_err()
    {
        tracing::warn!("document worker did not stop within the shutdown timeout");
    }

    result
}

async fn load_initial_license(pool: &sqlx::PgPool) -> Result<LicenseSnapshot, AppError> {
    #[derive(sqlx::FromRow)]
    struct InitialLicenseRow {
        institution_id: uuid::Uuid,
        deployment_id: uuid::Uuid,
        status: String,
        valid_from: chrono::DateTime<chrono::Utc>,
        valid_until: chrono::DateTime<chrono::Utc>,
        feature_set: serde_json::Value,
        version: i64,
    }

    // A deployment is intentionally single-tenant in this design. Refuse to
    // start without one explicit license row rather than silently running open.
    let row = sqlx::query_as::<_, InitialLicenseRow>(
        r#"
        SELECT
            institution_id, deployment_id, status,
            valid_from, valid_until, feature_set, version
        FROM institution_license
        ORDER BY updated_at DESC
        LIMIT 1
        "#,
    )
    .fetch_optional(pool)
    .await?
    .ok_or(AppError::InstitutionLocked)?;

    let status = match row.status.as_str() {
        "active" => LicenseStatus::Active,
        "suspended" => LicenseStatus::Suspended,
        "expired" => LicenseStatus::Expired,
        _ => return Err(AppError::Internal),
    };

    Ok(LicenseSnapshot {
        institution_id: row.institution_id,
        deployment_id: row.deployment_id,
        status,
        valid_from: row.valid_from,
        valid_until: row.valid_until,
        feature_set: row.feature_set,
        version: row.version,
    })
}
