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
use std::path::PathBuf;

use crate::audit::AuditWriter;
use crate::documents::{DocumentService, DocumentWorker};
use crate::enrollment::EnrollmentService;
use crate::licensing::{LicenseGate, LicenseService, LicenseSnapshot, LicenseStatus};
use crate::records::{GradeService, ScheduleQuery, TranscriptSnapshotService};
use crate::shared::error::AppError;
use actix_web::{App, HttpServer, middleware, web};
use sqlx::postgres::PgPoolOptions;

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter("info,actix_web=info,sqlx=warn")
        .json()
        .init();

    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");

    let pool = PgPoolOptions::new()
        // This is a bounded concurrency control, not a target to maximize.
        // Tune from database measurements.
        .max_connections(64)
        .min_connections(8)
        .connect(&database_url)
        .await
        .expect("database connection failed");

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("database migration failed");

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
        "document-worker-1".to_owned(),
        PathBuf::from("./var/documents"),
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
            .wrap(middleware::Logger::default())
            .configure(crate::app::recovery_routes)
            .configure(crate::app::protected_routes)
    })
    .workers(std::thread::available_parallelism().map_or(1, usize::from))
    .bind(("0.0.0.0", 8080))?
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
