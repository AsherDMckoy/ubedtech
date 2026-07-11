use crate::audit::AuditWriter;
use crate::shared::{actor::{Actor, Role}, error::AppError};
use sqlx::PgPool;
use uuid::Uuid;

use crate::licensing::{LicenseGate, LicenseSnapshot, LicenseStatus};

#[derive(Clone)]
pub struct LicenseService {
    pool: PgPool,
    gate: LicenseGate,
    audit: AuditWriter,
}

impl LicenseService {
    pub fn new(pool: PgPool, gate: LicenseGate, audit: AuditWriter) -> Self {
        Self { pool, gate, audit }
    }

    pub async fn set_status(
        &self,
        actor: &Actor,
        institution_id: Uuid,
        new_status: LicenseStatus,
        reason: &str,
    ) -> Result<LicenseSnapshot, AppError> {
        if !actor.has_role(Role::PlatformLicensingAdmin) {
            return Err(AppError::Forbidden);
        }

        if reason.trim().is_empty() {
            return Err(AppError::Validation("reason is required".into()));
        }

        let mut tx = self.pool.begin().await?;

        let old = sqlx::query_as::<_, LicenseRow>(
            r#"
            SELECT
                institution_id, deployment_id, status,
                valid_from, valid_until, feature_set, version
            FROM institution_license
            WHERE institution_id = $1
            FOR UPDATE
            "#,
        )
        .bind(institution_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(AppError::NotFound)?;

        let status_text = match new_status {
            LicenseStatus::Active => "active",
            LicenseStatus::Suspended => "suspended",
            LicenseStatus::Expired => "expired",
        };

        let updated = sqlx::query_as::<_, LicenseRow>(
            r#"
            UPDATE institution_license
               SET status = $2,
                   version = version + 1,
                   updated_at = now()
             WHERE institution_id = $1
            RETURNING
                institution_id, deployment_id, status,
                valid_from, valid_until, feature_set, version
            "#,
        )
        .bind(institution_id)
        .bind(status_text)
        .fetch_one(&mut *tx)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO license_change (
                id, institution_id, old_status, new_status,
                changed_by_user_id, reason
            )
            VALUES ($1, $2, $3, $4, $5, $6)
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(institution_id)
        .bind(&old.status)
        .bind(&updated.status)
        .bind(actor.user_id)
        .bind(reason)
        .execute(&mut *tx)
        .await?;

        self.audit
            .write(
                &mut tx,
                institution_id,
                actor.user_id,
                "license.status_changed",
                "institution_license",
                institution_id,
                &serde_json::json!({
                    "old": old.status,
                    "new": updated.status,
                    "reason": reason
                }),
            )
            .await?;

        tx.commit().await?;

        let snapshot = LicenseSnapshot::try_from(updated)?;

        // Replace only after the transaction commits. A request can observe either
        // the complete old state or the complete new state, never an uncommitted one.
        if self.gate.snapshot().institution_id == institution_id {
            self.gate.replace(snapshot.clone());
        }

        Ok(snapshot)
    }
}

#[derive(sqlx::FromRow)]
struct LicenseRow {
    institution_id: Uuid,
    deployment_id: Uuid,
    status: String,
    valid_from: chrono::DateTime<chrono::Utc>,
    valid_until: chrono::DateTime<chrono::Utc>,
    feature_set: serde_json::Value,
    version: i64,
}

impl TryFrom<LicenseRow> for LicenseSnapshot {
    type Error = AppError;

    fn try_from(row: LicenseRow) -> Result<Self, Self::Error> {
        let status = match row.status.as_str() {
            "active" => LicenseStatus::Active,
            "suspended" => LicenseStatus::Suspended,
            "expired" => LicenseStatus::Expired,
            _ => return Err(AppError::Internal),
        };

        Ok(Self {
            institution_id: row.institution_id,
            deployment_id: row.deployment_id,
            status,
            valid_from: row.valid_from,
            valid_until: row.valid_until,
            version: row.version,
            feature_set: row.feature_set,
        })
    }
}
