mod academics;
mod app;
mod audit;
mod config;
mod db;
mod documents;
mod enrollment;
mod identity_access;
mod institution;
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

    // Fail at startup, not mid-request, when the frontend bundles are
    // missing (ADR-12: dist/ is committed; APP_FRONTEND_DIST overrides).
    tracing::info!(
        css = crate::shared::assets::css_href(),
        js = crate::shared::assets::js_href(),
        "frontend assets loaded"
    );

    let pool = db::connect_and_migrate(&config)
        .await
        .expect("database connection or migration failed");

    // Operator subcommands run before any server machinery — in particular
    // before the license check, because bootstrapping the platform admin
    // must work on a deployment that is locked or not yet licensed.
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("bootstrap-platform-admin") {
        std::process::exit(run_bootstrap_platform_admin(&pool, &config, &args[2..]).await);
    }

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

    let academics = crate::academics::AcademicsService::new(pool.clone(), audit.clone());
    let enrollment = EnrollmentService::new(pool.clone(), audit.clone());
    let grades = GradeService::new(pool.clone(), audit.clone());
    let schedule = ScheduleQuery::new(pool.clone());
    let transcript = TranscriptSnapshotService;
    let documents = DocumentService::new(pool.clone(), audit.clone(), transcript);
    // The same store instance backs the worker's writes and the download
    // adapter's reads (the artifact-storage boundary; docs/OPERATIONS.md).
    let document_store = crate::documents::storage::FilesystemDocumentStore::new(
        config.document_storage_path.clone(),
    );
    let institution_admin =
        crate::institution::InstitutionService::new(pool.clone(), audit.clone());
    let licensing = LicenseService::new(pool.clone(), license_gate.clone(), audit);
    // Self-hosted deployments verify imported license files against this
    // public key; the private signing key never exists on a deployment.
    let import_key = crate::licensing::ImportVerifyingKey(config.license_public_key.map(|bytes| {
        ed25519_dalek::VerifyingKey::from_bytes(&bytes)
            .expect("APP_LICENSE_PUBLIC_KEY is not a valid Ed25519 public key")
    }));

    let worker = DocumentWorker::new(
        pool.clone(),
        config.worker_id.clone(),
        config.document_storage_path.clone(),
        config.job_stale_secs,
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
            .app_data(web::Data::new(academics.clone()))
            .app_data(web::Data::new(enrollment.clone()))
            .app_data(web::Data::new(grades.clone()))
            .app_data(web::Data::new(schedule.clone()))
            .app_data(web::Data::new(documents.clone()))
            .app_data(web::Data::new(institution_admin.clone()))
            .app_data(web::Data::new(document_store.clone()))
            .app_data(web::Data::new(TranscriptSnapshotService))
            .app_data(web::Data::new(licensing.clone()))
            .app_data(web::Data::new(import_key.clone()))
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
            // Compression sits outside everything that produces bodies; the
            // CSS/JS budget in docs/PERFORMANCE.md assumes it is on.
            .wrap(middleware::Compress::default())
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

/// `backend bootstrap-platform-admin <username> <email> [institution-code]`
///
/// Creates the first platform licensing admin (see identity_access::
/// bootstrap). The password comes from stdin — first line the password,
/// second line the confirmation — never from argv or the environment,
/// where it would leak into shell history and process listings.
async fn run_bootstrap_platform_admin(
    pool: &sqlx::PgPool,
    config: &AppConfig,
    args: &[String],
) -> i32 {
    let (username, email, institution_code) = match args {
        [username, email] => (username, email, None),
        [username, email, code] => (username, email, Some(code.as_str())),
        _ => {
            eprintln!(
                "usage: backend bootstrap-platform-admin <username> <email> [institution-code]\n\
                 The password is read from stdin: first line the password, \
                 second line the confirmation."
            );
            return 2;
        }
    };

    use std::io::{BufRead, IsTerminal};
    let stdin = std::io::stdin();
    if stdin.is_terminal() {
        // No unechoed prompt without a new dependency; steer operators to
        // the pipe form documented in docs/OPERATIONS.md instead.
        eprintln!(
            "warning: typing the password here will echo it; \
             prefer piping it in (see docs/OPERATIONS.md)"
        );
        eprintln!("password (then confirmation on the next line):");
    }
    let mut lines = stdin.lock().lines();
    let (Some(Ok(password)), Some(Ok(confirmation))) = (lines.next(), lines.next()) else {
        eprintln!("bootstrap failed: could not read the password and confirmation from stdin");
        return 2;
    };
    if password != confirmation {
        eprintln!("bootstrap failed: password and confirmation do not match");
        return 2;
    }

    let passwords = match PasswordService::from_config(config) {
        Ok(passwords) => passwords,
        Err(error) => {
            eprintln!("bootstrap failed: invalid Argon2 parameters: {error}");
            return 1;
        }
    };

    match crate::identity_access::bootstrap::bootstrap_platform_admin(
        pool,
        &passwords,
        username,
        email,
        &password,
        institution_code,
    )
    .await
    {
        Ok(user_id) => {
            // AppError Display never carries secrets, and neither does this.
            println!("platform licensing admin created: user {user_id}");
            0
        }
        Err(error) => {
            eprintln!("bootstrap failed: {error}");
            1
        }
    }
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
