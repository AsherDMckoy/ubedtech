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
    actix_web::rt::spawn(worker.run());

    HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(pool.clone()))
            .app_data(web::Data::new(enrollment.clone()))
            .app_data(web::Data::new(grades.clone()))
            .app_data(web::Data::new(schedule.clone()))
            .app_data(web::Data::new(documents.clone()))
            .app_data(web::Data::new(licensing.clone()))
            .app_data(web::Data::new(license_gate.clone()))
            .wrap(middleware::NormalizePath::trim())
            .wrap(middleware::DefaultHeaders::new()
                .add(("X-Content-Type-Options", "nosniff"))
                .add(("Referrer-Policy", "strict-origin-when-cross-origin"))
                .add(("Content-Security-Policy",
                      "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'")))
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
    .run()
    .await
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
