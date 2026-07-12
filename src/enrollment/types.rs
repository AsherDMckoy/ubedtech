use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Deserialize)]
pub struct RegisterCommand {
    pub section_id: Uuid,
    pub idempotency_key: Uuid,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct EnrollmentReceipt {
    pub enrollment_id: Uuid,
    pub section_id: Uuid,
    pub status: String,
    pub registered_at: DateTime<Utc>,
}

#[derive(Debug, sqlx::FromRow)]
pub struct RegistrationContext {
    pub term_id: Uuid,
    pub course_id: Uuid,
    pub section_status: String,
    pub registration_opens_at: DateTime<Utc>,
    pub registration_closes_at: DateTime<Utc>,
    #[allow(dead_code)] // read by the Phase 4.1 add-during-drop/add window policy
    pub drop_add_closes_at: DateTime<Utc>,
}
