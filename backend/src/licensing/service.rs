use crate::audit::AuditWriter;
use crate::shared::{
    actor::{Actor, Role},
    error::AppError,
};
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

        // Self-hosted deployments take license state from the signed file
        // only — a manual flip here would silently diverge from what the
        // signature attests. Mode is immutable, so reading it outside the
        // transaction is safe.
        let mode: String =
            sqlx::query_scalar("SELECT mode FROM institution_license WHERE institution_id = $1")
                .bind(institution_id)
                .fetch_optional(&self.pool)
                .await?
                .ok_or(AppError::NotFound)?;
        if mode == "self_hosted" {
            return Err(AppError::Validation(
                "self-hosted license state changes only through signed license import".into(),
            ));
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

impl LicenseService {
    /// Self-hosted recovery path: apply a platform-signed license file.
    /// The signature is the platform's authority; the authenticated admin
    /// (institution or platform) is who we attribute the import to. The
    /// license row update, the change record, and the audit event commit in
    /// one transaction; the gate snapshot swaps only after commit.
    pub async fn import_signed(
        &self,
        actor: &Actor,
        key: Option<&ed25519_dalek::VerifyingKey>,
        file: &crate::licensing::signed_license::SignedLicenseFile,
    ) -> Result<LicenseSnapshot, AppError> {
        if !actor.has_role(Role::InstitutionAdmin) && !actor.has_role(Role::PlatformLicensingAdmin)
        {
            return Err(AppError::Forbidden);
        }
        let Some(key) = key else {
            return Err(AppError::Validation(
                "this deployment does not accept signed license imports".into(),
            ));
        };

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
        .bind(actor.institution_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(AppError::NotFound)?;

        let claims =
            crate::licensing::signed_license::verify_signed_license(file, key, old.deployment_id)?;
        if claims.institution_id != old.institution_id {
            return Err(AppError::Validation(
                "license is for a different institution".into(),
            ));
        }

        let updated = sqlx::query_as::<_, LicenseRow>(
            r#"
            UPDATE institution_license
               SET status = 'active',
                   valid_from = $2,
                   valid_until = $3,
                   feature_set = $4,
                   version = version + 1,
                   updated_at = now()
             WHERE institution_id = $1
            RETURNING
                institution_id, deployment_id, status,
                valid_from, valid_until, feature_set, version
            "#,
        )
        .bind(old.institution_id)
        .bind(claims.valid_from)
        .bind(claims.valid_until)
        .bind(&claims.feature_set)
        .fetch_one(&mut *tx)
        .await?;

        let reason = format!("signed license import (serial {})", claims.license_serial);
        sqlx::query(
            r#"
            INSERT INTO license_change (
                id, institution_id, old_status, new_status,
                changed_by_user_id, reason
            )
            VALUES ($1, $2, $3, 'active', $4, $5)
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(old.institution_id)
        .bind(&old.status)
        .bind(actor.user_id)
        .bind(&reason)
        .execute(&mut *tx)
        .await?;

        self.audit
            .write(
                &mut tx,
                old.institution_id,
                actor.user_id,
                "license.imported",
                "institution_license",
                old.institution_id,
                &serde_json::json!({
                    "license_serial": claims.license_serial,
                    "valid_from": claims.valid_from,
                    "valid_until": claims.valid_until,
                    "old_status": old.status,
                }),
            )
            .await?;

        tx.commit().await?;

        let snapshot = LicenseSnapshot::try_from(updated)?;
        if self.gate.snapshot().institution_id == snapshot.institution_id {
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
