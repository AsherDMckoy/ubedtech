use crate::audit::AuditWriter;
use crate::records::TranscriptSnapshotService;
use crate::shared::{
    actor::{Actor, Role},
    error::AppError,
};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Clone)]
pub struct DocumentService {
    pool: PgPool,
    audit: AuditWriter,
    transcript_snapshots: TranscriptSnapshotService,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RequestDocumentCommand {
    pub document_type: String,
    pub purpose: Option<String>,
    pub delivery_method: String,
}

#[derive(Debug, Serialize)]
pub struct DocumentRequestReceipt {
    pub request_id: Uuid,
    pub status: &'static str,
}

impl DocumentService {
    pub fn new(
        pool: PgPool,
        audit: AuditWriter,
        transcript_snapshots: TranscriptSnapshotService,
    ) -> Self {
        Self {
            pool,
            audit,
            transcript_snapshots,
        }
    }

    pub async fn request_for_self(
        &self,
        actor: &Actor,
        command: RequestDocumentCommand,
    ) -> Result<DocumentRequestReceipt, AppError> {
        let student_id = actor.require_student_self()?;

        validate_request(&command)?;

        let request_id = Uuid::new_v4();
        let mut tx = self.pool.begin().await?;

        sqlx::query(
            r#"
            INSERT INTO document_request (
                id, institution_id, student_id, document_type,
                status, purpose, delivery_method
            )
            VALUES ($1, $2, $3, $4, 'pending', $5, $6)
            "#,
        )
        .bind(request_id)
        .bind(actor.institution_id)
        .bind(student_id)
        .bind(&command.document_type)
        .bind(command.purpose.as_deref())
        .bind(&command.delivery_method)
        .execute(&mut *tx)
        .await?;

        self.audit
            .write(
                &mut tx,
                actor.institution_id,
                actor.user_id,
                "document.requested",
                "document_request",
                request_id,
                &command,
            )
            .await?;

        tx.commit().await?;

        Ok(DocumentRequestReceipt {
            request_id,
            status: "pending",
        })
    }

    pub async fn approve(
        &self,
        actor: &Actor,
        request_id: Uuid,
        note: Option<&str>,
    ) -> Result<(), AppError> {
        if !actor.has_role(Role::DocumentOfficer) {
            return Err(AppError::Forbidden);
        }

        let mut tx = self.pool.begin().await?;

        let request = sqlx::query_as::<_, PendingRequest>(
            r#"
            SELECT student_id, document_type
            FROM document_request
            WHERE id = $1
              AND institution_id = $2
              AND status = 'pending'
            FOR UPDATE
            "#,
        )
        .bind(request_id)
        .bind(actor.institution_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(AppError::NotFound)?;

        let snapshot_id = match request.document_type.as_str() {
            "official_transcript" | "enrollment_letter" => Some(
                self.transcript_snapshots
                    .create(&mut tx, actor.institution_id, request.student_id)
                    .await?,
            ),
            "signed_document" => None,
            _ => return Err(AppError::Validation("unknown document type".into())),
        };

        sqlx::query(
            r#"
            INSERT INTO document_approval (
                id, request_id, decision, decided_by_user_id, note
            )
            VALUES ($1, $2, 'approved', $3, $4)
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(request_id)
        .bind(actor.user_id)
        .bind(note)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            r#"
            UPDATE document_request
               SET status = 'approved',
                   current_snapshot_id = $2,
                   updated_at = now()
             WHERE id = $1
            "#,
        )
        .bind(request_id)
        .bind(snapshot_id)
        .execute(&mut *tx)
        .await?;

        // The job is created in the same transaction. An approved request cannot
        // exist without durable work queued to produce its artifact.
        sqlx::query(
            r#"
            INSERT INTO document_job (id, request_id, status)
            VALUES ($1, $2, 'queued')
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(request_id)
        .execute(&mut *tx)
        .await?;

        self.audit
            .write(
                &mut tx,
                actor.institution_id,
                actor.user_id,
                "document.approved",
                "document_request",
                request_id,
                &serde_json::json!({ "snapshot_id": snapshot_id, "note": note }),
            )
            .await?;

        tx.commit().await?;
        Ok(())
    }

    pub async fn reject(
        &self,
        actor: &Actor,
        request_id: Uuid,
        note: &str,
    ) -> Result<(), AppError> {
        if !actor.has_role(Role::DocumentOfficer) {
            return Err(AppError::Forbidden);
        }

        if note.trim().is_empty() {
            return Err(AppError::Validation(
                "a rejection reason is required".into(),
            ));
        }

        let mut tx = self.pool.begin().await?;

        let changed = sqlx::query(
            r#"
            UPDATE document_request
               SET status = 'rejected', updated_at = now()
             WHERE id = $1
               AND institution_id = $2
               AND status = 'pending'
            "#,
        )
        .bind(request_id)
        .bind(actor.institution_id)
        .execute(&mut *tx)
        .await?;

        if changed.rows_affected() != 1 {
            return Err(AppError::NotFound);
        }

        sqlx::query(
            r#"
            INSERT INTO document_approval (
                id, request_id, decision, decided_by_user_id, note
            )
            VALUES ($1, $2, 'rejected', $3, $4)
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(request_id)
        .bind(actor.user_id)
        .bind(note)
        .execute(&mut *tx)
        .await?;

        self.audit
            .write(
                &mut tx,
                actor.institution_id,
                actor.user_id,
                "document.rejected",
                "document_request",
                request_id,
                &serde_json::json!({ "note": note }),
            )
            .await?;

        tx.commit().await?;
        Ok(())
    }
}

#[derive(sqlx::FromRow)]
struct PendingRequest {
    student_id: Uuid,
    document_type: String,
}

fn validate_request(command: &RequestDocumentCommand) -> Result<(), AppError> {
    match command.document_type.as_str() {
        "official_transcript" | "enrollment_letter" | "signed_document" => {}
        _ => return Err(AppError::Validation("unknown document type".into())),
    }

    match command.delivery_method.as_str() {
        "download" | "pickup" | "email" => {}
        _ => return Err(AppError::Validation("unknown delivery method".into())),
    }

    if command
        .purpose
        .as_deref()
        .is_some_and(|value| value.len() > 500)
    {
        return Err(AppError::Validation("purpose is too long".into()));
    }

    Ok(())
}
