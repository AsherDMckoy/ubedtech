use std::time::Duration;

use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

use crate::config::AppConfig;

/// Build the single bounded application pool and bring the schema up to date.
/// Called once at startup; a failure here must abort the process, so the
/// caller decides how to exit — this function only reports.
pub async fn connect_and_migrate(config: &AppConfig) -> Result<PgPool, sqlx::Error> {
    let pool = PgPoolOptions::new()
        // A bounded pool is deliberate backpressure: a traffic spike queues at
        // acquire (bounded by acquire_timeout) instead of collapsing PostgreSQL.
        // Tune the bounds from database measurements, not from guesses.
        .max_connections(config.db_max_connections)
        .min_connections(config.db_min_connections)
        .acquire_timeout(Duration::from_secs(config.db_acquire_timeout_secs))
        .connect(&config.database_url)
        .await?;

    sqlx::migrate!("./migrations").run(&pool).await?;

    Ok(pool)
}
