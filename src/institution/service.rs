//! Institution administration: events/holidays, settings, and document-type
//! configuration. Every mutation is institution-admin only, scoped to the
//! actor's own institution, and audited in the same transaction — this
//! service holds no special powers over enrollment, grades, records, or
//! documents, which keep their own rules.

use crate::audit::AuditWriter;
use crate::shared::{
    actor::{Actor, Role},
    error::AppError,
};
use chrono::NaiveDate;
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Clone)]
pub struct InstitutionService {
    pool: PgPool,
    audit: AuditWriter,
}

#[derive(Debug, Serialize)]
pub struct EventCommand {
    pub title: String,
    pub event_type: String,
    pub starts_on: NaiveDate,
    pub ends_on: NaiveDate,
}

#[derive(sqlx::FromRow)]
pub struct InstitutionEvent {
    pub id: Uuid,
    pub title: String,
    pub event_type: String,
    pub starts_on: NaiveDate,
    pub ends_on: NaiveDate,
}

impl InstitutionService {
    pub fn new(pool: PgPool, audit: AuditWriter) -> Self {
        Self { pool, audit }
    }

    pub async fn create_event(
        &self,
        actor: &Actor,
        command: EventCommand,
    ) -> Result<Uuid, AppError> {
        require_institution_admin(actor)?;

        let title = command.title.trim();
        if title.is_empty() {
            return Err(AppError::Validation("a title is required".into()));
        }
        if title.len() > 200 {
            return Err(AppError::Validation("title is too long".into()));
        }
        match command.event_type.as_str() {
            "holiday" | "event" => {}
            _ => return Err(AppError::Validation("unknown event type".into())),
        }
        if command.starts_on > command.ends_on {
            return Err(AppError::Validation(
                "the event may not end before it starts".into(),
            ));
        }

        let event_id = Uuid::new_v4();
        let mut tx = self.pool.begin().await?;

        sqlx::query(
            r#"
            INSERT INTO institution_event (
                id, institution_id, title, event_type,
                starts_on, ends_on, created_by_user_id
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            "#,
        )
        .bind(event_id)
        .bind(actor.institution_id)
        .bind(title)
        .bind(&command.event_type)
        .bind(command.starts_on)
        .bind(command.ends_on)
        .bind(actor.user_id)
        .execute(&mut *tx)
        .await
        .map_err(|error| {
            conflict_on_unique(
                error,
                "an event with this title and start date already exists",
            )
        })?;

        self.audit
            .write(
                &mut tx,
                actor.institution_id,
                actor.user_id,
                "institution.event_created",
                "institution_event",
                event_id,
                &command,
            )
            .await?;

        tx.commit().await?;
        Ok(event_id)
    }

    pub async fn delete_event(&self, actor: &Actor, event_id: Uuid) -> Result<(), AppError> {
        require_institution_admin(actor)?;

        let mut tx = self.pool.begin().await?;

        // Scoped delete: an admin can only ever remove their own
        // institution's events; anything else looks nonexistent.
        let title: Option<String> = sqlx::query_scalar(
            r#"
            DELETE FROM institution_event
            WHERE id = $1 AND institution_id = $2
            RETURNING title
            "#,
        )
        .bind(event_id)
        .bind(actor.institution_id)
        .fetch_optional(&mut *tx)
        .await?;
        let title = title.ok_or(AppError::NotFound)?;

        self.audit
            .write(
                &mut tx,
                actor.institution_id,
                actor.user_id,
                "institution.event_deleted",
                "institution_event",
                event_id,
                &serde_json::json!({ "title": title }),
            )
            .await?;

        tx.commit().await?;
        Ok(())
    }

    pub async fn list_events(&self, actor: &Actor) -> Result<Vec<InstitutionEvent>, AppError> {
        require_institution_admin(actor)?;

        Ok(sqlx::query_as::<_, InstitutionEvent>(
            r#"
            SELECT id, title, event_type, starts_on, ends_on
            FROM institution_event
            WHERE institution_id = $1
            ORDER BY starts_on, title
            LIMIT 200
            "#,
        )
        .bind(actor.institution_id)
        .fetch_all(&self.pool)
        .await?)
    }
}

fn require_institution_admin(actor: &Actor) -> Result<(), AppError> {
    if actor.has_role(Role::InstitutionAdmin) {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

/// Unique-constraint violations are user-visible conflicts, not internal
/// errors (same mapping as academics).
fn conflict_on_unique(error: sqlx::Error, message: &str) -> AppError {
    if error
        .as_database_error()
        .and_then(|db| db.code())
        .is_some_and(|code| code == "23505")
    {
        AppError::Conflict(message.into())
    } else {
        AppError::Database(error)
    }
}
