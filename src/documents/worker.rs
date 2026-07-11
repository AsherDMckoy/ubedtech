use std::{path::PathBuf, time::Duration};

use crate::shared::error::AppError;
use printpdf::{
    BuiltinFont, Mm, Op, PdfDocument, PdfFontHandle, PdfPage, PdfSaveOptions,
    Point, Pt, TextItem,
};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tokio::time::sleep;
use uuid::Uuid;

use crate::documents::storage::FilesystemDocumentStore;

#[derive(Clone)]
pub struct DocumentWorker {
    pool: PgPool,
    worker_id: String,
    store: FilesystemDocumentStore,
}

impl DocumentWorker {
    pub fn new(pool: PgPool, worker_id: String, root: PathBuf) -> Self {
        Self {
            pool,
            worker_id,
            store: FilesystemDocumentStore::new(root),
        }
    }

    pub async fn run(self) {
        loop {
            match self.run_once().await {
                Ok(true) => {}
                Ok(false) => sleep(Duration::from_millis(200)).await,
                Err(error) => {
                    tracing::error!(?error, "document worker iteration failed");
                    sleep(Duration::from_secs(1)).await;
                }
            }
        }
    }

    async fn run_once(&self) -> Result<bool, AppError> {
        let mut tx = self.pool.begin().await?;

        let job = sqlx::query_as::<_, ClaimedJob>(
            r#"
            SELECT j.id AS job_id, j.request_id, r.current_snapshot_id
            FROM document_job j
            JOIN document_request r ON r.id = j.request_id
            WHERE j.status = 'queued'
              AND j.available_at <= now()
            ORDER BY j.created_at
            FOR UPDATE OF j SKIP LOCKED
            LIMIT 1
            "#,
        )
        .fetch_optional(&mut *tx)
        .await?;

        let Some(job) = job else {
            tx.rollback().await?;
            return Ok(false);
        };

        sqlx::query(
            r#"
            UPDATE document_job
               SET status = 'running',
                   locked_at = now(),
                   locked_by = $2,
                   attempts = attempts + 1
             WHERE id = $1
            "#,
        )
        .bind(job.job_id)
        .bind(&self.worker_id)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "UPDATE document_request SET status = 'generating', updated_at = now() WHERE id = $1",
        )
        .bind(job.request_id)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        // Rendering is deliberately outside the transaction. Never hold a DB
        // lock while doing PDF work or filesystem I/O.
        let result = self.generate(job.request_id, job.current_snapshot_id).await;

        match result {
            Ok(artifact) => self.complete(job, artifact).await?,
            Err(error) => self.fail(job.job_id, job.request_id, &error.to_string()).await?,
        }

        Ok(true)
    }

    async fn generate(
        &self,
        request_id: Uuid,
        snapshot_id: Option<Uuid>,
    ) -> Result<Artifact, AppError> {
        let snapshot: Option<serde_json::Value> = match snapshot_id {
            Some(id) => sqlx::query_scalar(
                "SELECT snapshot_json FROM transcript_snapshot WHERE id = $1",
            )
            .bind(id)
            .fetch_optional(&self.pool)
            .await?,
            None => None,
        };

        // The demo adapter emits a valid minimal PDF. Production replaces only
        // the renderer with approved stationery, fonts, seals, and signatures.
        let pdf_bytes = render_pdf(request_id, snapshot.as_ref())?;
        let digest = Sha256::digest(&pdf_bytes);
        let hash_hex = hex::encode(digest);
        let path = self.store.write(&hash_hex, &pdf_bytes).await?;

        Ok(Artifact {
            bytes: pdf_bytes.len() as i64,
            hash: digest.to_vec(),
            path,
        })
    }

    async fn complete(&self, job: ClaimedJob, artifact: Artifact) -> Result<(), AppError> {
        let mut tx = self.pool.begin().await?;

        sqlx::query(
            r#"
            INSERT INTO generated_document (
                id, request_id, snapshot_id, content_hash,
                storage_path, mime_type, size_bytes
            )
            VALUES ($1, $2, $3, $4, $5, 'application/pdf', $6)
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(job.request_id)
        .bind(job.current_snapshot_id)
        .bind(artifact.hash)
        .bind(artifact.path)
        .bind(artifact.bytes)
        .execute(&mut *tx)
        .await?;

        sqlx::query("UPDATE document_job SET status = 'complete' WHERE id = $1")
            .bind(job.job_id)
            .execute(&mut *tx)
            .await?;

        sqlx::query(
            "UPDATE document_request SET status = 'ready', updated_at = now() WHERE id = $1",
        )
        .bind(job.request_id)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(())
    }

    async fn fail(
        &self,
        job_id: Uuid,
        request_id: Uuid,
        message: &str,
    ) -> Result<(), AppError> {
        let mut tx = self.pool.begin().await?;

        sqlx::query(
            r#"
            UPDATE document_job
               SET status = CASE WHEN attempts >= 3 THEN 'failed' ELSE 'queued' END,
                   available_at = CASE
                       WHEN attempts >= 3 THEN available_at
                       ELSE now() + interval '30 seconds'
                   END,
                   last_error = $2,
                   locked_at = NULL,
                   locked_by = NULL
             WHERE id = $1
            "#,
        )
        .bind(job_id)
        .bind(truncate_for_log(message, 1000))
        .execute(&mut *tx)
        .await?;

        let terminal: bool = sqlx::query_scalar(
            "SELECT status = 'failed' FROM document_job WHERE id = $1",
        )
        .bind(job_id)
        .fetch_one(&mut *tx)
        .await?;

        if terminal {
            sqlx::query(
                "UPDATE document_request SET status = 'failed', updated_at = now() WHERE id = $1",
            )
            .bind(request_id)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }
}

#[derive(sqlx::FromRow, Clone, Copy)]
struct ClaimedJob {
    job_id: Uuid,
    request_id: Uuid,
    current_snapshot_id: Option<Uuid>,
}

struct Artifact {
    bytes: i64,
    hash: Vec<u8>,
    path: String,
}

fn render_pdf(
    request_id: Uuid,
    snapshot: Option<&serde_json::Value>,
) -> Result<Vec<u8>, AppError> {
    // This is a valid, deliberately plain demo renderer. It proves the worker,
    // artifact, hashing, and download path without coupling the documents
    // module to a browser or office-suite process. Replace this function—not
    // the workflow—with a versioned University template before production.
    let snapshot = snapshot.ok_or(AppError::Internal)?;

    let string_field = |name: &str| {
        snapshot
            .get(name)
            .and_then(serde_json::Value::as_str)
            .unwrap_or("Not available")
            .to_owned()
    };

    let mut lines: Vec<PdfLine> = vec![
        PdfLine::title("University of Belize"),
        PdfLine::subtitle("Official Transcript — Demo Layout"),
        PdfLine::body(format!("Request ID: {request_id}")),
        PdfLine::body(format!("Student: {}", string_field("student_name"))),
        PdfLine::body(format!("Student number: {}", string_field("student_number"))),
        PdfLine::body(format!("Program: {}", string_field("program_code"))),
        PdfLine::body(""),
        PdfLine::heading("Academic record"),
    ];

    if let Some(courses) = snapshot.get("courses").and_then(serde_json::Value::as_array) {
        for course in courses {
            let term = json_text(course, "term_code");
            let code = json_text(course, "course_code");
            let title = json_text(course, "course_title");
            let grade = json_text(course, "grade_code");
            let credits = course
                .get("credit_hours")
                .and_then(serde_json::Value::as_f64)
                .map(|value| format!("{value:.1}"))
                .unwrap_or_else(|| "—".to_owned());

            lines.extend(wrap_pdf_line(
                format!("{term} | {code} | {title} | {credits} credits | Grade {grade}"),
                92,
            ));
        }
    }

    lines.push(PdfLine::body(""));
    lines.push(PdfLine::body(
        "Generated from an immutable approved snapshot. Verify the artifact hash before external use.",
    ));

    const LINES_PER_PAGE: usize = 43;
    let mut pages = Vec::new();

    for (page_index, chunk) in lines.chunks(LINES_PER_PAGE).enumerate() {
        let mut ops = vec![
            Op::StartTextSection,
            Op::SetTextCursor {
                pos: Point::new(Mm(18.0), Mm(279.0)),
            },
            Op::SetLineHeight { lh: Pt(15.0) },
        ];

        if page_index > 0 {
            ops.extend([
                Op::SetFont {
                    font: PdfFontHandle::Builtin(BuiltinFont::HelveticaBold),
                    size: Pt(11.0),
                },
                Op::ShowText {
                    items: vec![TextItem::Text("Official Transcript — continued".to_owned())],
                },
                Op::AddLineBreak,
                Op::AddLineBreak,
            ]);
        }

        for line in chunk {
            ops.push(Op::SetFont {
                font: PdfFontHandle::Builtin(line.font),
                size: Pt(line.size),
            });
            ops.push(Op::ShowText {
                items: vec![TextItem::Text(line.text.clone())],
            });
            ops.push(Op::AddLineBreak);
        }

        ops.push(Op::EndTextSection);
        pages.push(PdfPage::new(Mm(210.0), Mm(297.0), ops));
    }

    let mut document = PdfDocument::new("University of Belize Official Transcript");
    document.with_pages(pages);

    let mut warnings = Vec::new();
    let bytes = document.save(&PdfSaveOptions::default(), &mut warnings);

    for warning in warnings {
        tracing::warn!(?warning, %request_id, "PDF renderer warning");
    }

    if !bytes.starts_with(b"%PDF-") {
        return Err(AppError::Internal);
    }

    Ok(bytes)
}

#[derive(Clone)]
struct PdfLine {
    text: String,
    font: BuiltinFont,
    size: f32,
}

impl PdfLine {
    fn title(text: impl Into<String>) -> Self {
        Self { text: text.into(), font: BuiltinFont::HelveticaBold, size: 18.0 }
    }

    fn subtitle(text: impl Into<String>) -> Self {
        Self { text: text.into(), font: BuiltinFont::HelveticaBold, size: 13.0 }
    }

    fn heading(text: impl Into<String>) -> Self {
        Self { text: text.into(), font: BuiltinFont::HelveticaBold, size: 11.0 }
    }

    fn body(text: impl Into<String>) -> Self {
        Self { text: text.into(), font: BuiltinFont::Helvetica, size: 10.0 }
    }
}

fn json_text(value: &serde_json::Value, key: &str) -> String {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or("—")
        .to_owned()
}

fn wrap_pdf_line(text: String, maximum_chars: usize) -> Vec<PdfLine> {
    let mut result = Vec::new();
    let mut current = String::new();

    for word in text.split_whitespace() {
        let needed = current.len() + usize::from(!current.is_empty()) + word.len();
        if needed > maximum_chars && !current.is_empty() {
            result.push(PdfLine::body(std::mem::take(&mut current)));
        }

        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }

    if !current.is_empty() {
        result.push(PdfLine::body(current));
    }

    if result.is_empty() {
        result.push(PdfLine::body(""));
    }

    result
}

fn truncate_for_log(value: &str, maximum: usize) -> &str {
    value.get(..maximum).unwrap_or(value)
}
