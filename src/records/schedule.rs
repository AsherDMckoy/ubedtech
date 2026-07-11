use crate::shared::{actor::Actor, error::AppError};
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Clone)]
pub struct ScheduleQuery {
    pool: PgPool,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct ScheduleMeeting {
    pub course_code: String,
    pub course_title: String,
    pub section_code: String,
    pub day_of_week: i16,
    pub starts_at: chrono::NaiveTime,
    pub ends_at: chrono::NaiveTime,
    pub campus_code: Option<String>,
    pub room_code: Option<String>,
}

impl ScheduleQuery {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn for_student(
        &self,
        actor: &Actor,
        term_id: Uuid,
    ) -> Result<Vec<ScheduleMeeting>, AppError> {
        let student_id = actor.require_student_self()?;

        let meetings = sqlx::query_as::<_, ScheduleMeeting>(
            r#"
            SELECT
                c.code AS course_code,
                c.title AS course_title,
                s.section_code,
                m.day_of_week,
                m.starts_at,
                m.ends_at,
                r.campus_code,
                r.room_code
            FROM enrollment e
            JOIN section s ON s.id = e.section_id
            JOIN course c ON c.id = s.course_id
            JOIN section_meeting m ON m.section_id = s.id
            LEFT JOIN room r ON r.id = m.room_id
            WHERE e.student_id = $1
              AND e.institution_id = $2
              AND e.status = 'enrolled'
              AND s.term_id = $3
            ORDER BY m.day_of_week, m.starts_at, c.code
            "#,
        )
        .bind(student_id)
        .bind(actor.institution_id)
        .bind(term_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(meetings)
    }
}
