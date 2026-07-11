use crate::shared::error::AppError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

#[derive(Clone, Default)]
pub struct TranscriptSnapshotService;

#[derive(Debug, Serialize, Deserialize)]
pub struct TranscriptSnapshotData {
    pub student_number: String,
    pub student_name: String,
    pub program_code: String,
    pub courses: Vec<TranscriptCourse>,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct TranscriptCourse {
    pub term_code: String,
    pub course_code: String,
    pub course_title: String,
    pub credit_hours: f64,
    pub grade_code: String,
    pub grade_points: Option<f64>,
}

impl TranscriptSnapshotService {
    pub async fn create(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        institution_id: Uuid,
        student_id: Uuid,
    ) -> Result<Uuid, AppError> {
        // Snapshot versions are monotonic per student. Lock the stable student
        // row before calculating max(version)+1 so two approvals cannot choose
        // the same version concurrently.
        sqlx::query_scalar::<_, i32>(
            r#"
            SELECT 1
            FROM student_profile
            WHERE id = $1 AND institution_id = $2
            FOR UPDATE
            "#,
        )
        .bind(student_id)
        .bind(institution_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(AppError::NotFound)?;

        let student = sqlx::query_as::<_, StudentHeader>(
            r#"
            SELECT
                sp.student_number,
                ua.username AS student_name,
                sp.program_code
            FROM student_profile sp
            JOIN user_account ua ON ua.id = sp.user_id
            WHERE sp.id = $1 AND sp.institution_id = $2
            "#,
        )
        .bind(student_id)
        .bind(institution_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(AppError::NotFound)?;

        let courses = sqlx::query_as::<_, TranscriptCourse>(
            r#"
            SELECT
                t.code AS term_code,
                c.code AS course_code,
                c.title AS course_title,
                c.credit_hours::float8 AS credit_hours,
                g.grade_code,
                g.grade_points
            FROM grade_record g
            JOIN enrollment e ON e.id = g.enrollment_id
            JOIN section s ON s.id = e.section_id
            JOIN academic_term t ON t.id = s.term_id
            JOIN course c ON c.id = s.course_id
            WHERE e.student_id = $1
              AND g.institution_id = $2
              AND g.state IN ('published', 'amended')
            ORDER BY t.starts_on, c.code
            "#,
        )
        .bind(student_id)
        .bind(institution_id)
        .fetch_all(&mut **tx)
        .await?;

        let data = TranscriptSnapshotData {
            student_number: student.student_number,
            student_name: student.student_name,
            program_code: student.program_code,
            courses,
        };

        let bytes = serde_json::to_vec(&data).map_err(|_| AppError::Internal)?;
        let hash = Sha256::digest(&bytes).to_vec();

        let next_version: i64 = sqlx::query_scalar(
            r#"
            SELECT COALESCE(max(snapshot_version), 0) + 1
            FROM transcript_snapshot
            WHERE institution_id = $1 AND student_id = $2
            "#,
        )
        .bind(institution_id)
        .bind(student_id)
        .fetch_one(&mut **tx)
        .await?;

        let snapshot_id = Uuid::new_v4();
        sqlx::query(
            r#"
            INSERT INTO transcript_snapshot (
                id, institution_id, student_id, snapshot_version,
                snapshot_json, content_hash
            )
            VALUES ($1, $2, $3, $4, $5, $6)
            "#,
        )
        .bind(snapshot_id)
        .bind(institution_id)
        .bind(student_id)
        .bind(next_version)
        .bind(sqlx::types::Json(&data))
        .bind(hash)
        .execute(&mut **tx)
        .await?;

        Ok(snapshot_id)
    }
}

#[derive(sqlx::FromRow)]
struct StudentHeader {
    student_number: String,
    student_name: String,
    program_code: String,
}

