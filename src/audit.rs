use chrono::Utc;
use serde::Serialize;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

#[derive(Clone, Default)]
pub struct AuditWriter;

impl AuditWriter {
    pub async fn write<T: Serialize>(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        institution_id: Uuid,
        actor_user_id: Uuid,
        action: &str,
        resource_type: &str,
        resource_id: Uuid,
        detail: &T,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            INSERT INTO audit_event (
                id, institution_id, actor_user_id, action,
                resource_type, resource_id, detail, occurred_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            "#,
        )
            .bind(Uuid::new_v4())
            .bind(institution_id)
            .bind(actor_user_id)
            .bind(action)
            .bind(resource_type)
            .bind(resource_id)
            .bind(sqlx::types::Json(detail))
            .bind(Utc::now())
            .execute(&mut **tx)
            .await?;

        Ok(())
    }
}
