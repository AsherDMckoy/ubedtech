use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::shared::error::AppError;

#[derive(Debug, Deserialize)]
pub struct RegisterCommand {
    pub section_id: Uuid,
    pub idempotency_key: Uuid,
}

/// Every reason the enrollment service refuses a registration or drop, as a
/// type rather than a message string. The UI renders one inline explanation
/// per variant; tests assert the exact reason instead of parsing prose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Denial {
    SectionNotOpen,
    WindowClosed,
    Hold,
    AlreadyEnrolled,
    PrerequisiteNotMet,
    ScheduleConflict,
    SectionFull,
}

impl Denial {
    pub fn message(self) -> &'static str {
        match self {
            Denial::SectionNotOpen => "section is not open",
            Denial::WindowClosed => "registration window is closed",
            Denial::Hold => "student has a registration hold",
            Denial::AlreadyEnrolled => "student is already enrolled in this section",
            Denial::PrerequisiteNotMet => "prerequisite requirements are not satisfied",
            Denial::ScheduleConflict => "schedule conflict detected",
            Denial::SectionFull => "section is full",
        }
    }
}

/// Enrollment use-case error: a typed business denial, or anything else the
/// shared error covers. Converts into `AppError` (denials become 409
/// Conflicts) so JSON handlers keep their behavior, while the HTML handlers
/// match on `Denied` to render inline feedback per case.
#[derive(Debug, thiserror::Error)]
pub enum EnrollError {
    #[error("{}", .0.message())]
    Denied(Denial),
    #[error(transparent)]
    App(#[from] AppError),
}

impl From<sqlx::Error> for EnrollError {
    fn from(error: sqlx::Error) -> Self {
        EnrollError::App(AppError::from(error))
    }
}

impl From<EnrollError> for AppError {
    fn from(error: EnrollError) -> Self {
        match error {
            EnrollError::Denied(denial) => AppError::Conflict(denial.message().into()),
            EnrollError::App(inner) => inner,
        }
    }
}

/// Registrar command creating a registration override: the full record
/// demanded by CLAUDE.md §1 item 6 — who (the actor), which rule, why
/// (required reason), expiry, and later which enrollment consumed it.
#[derive(Debug, Deserialize)]
pub struct GrantOverrideCommand {
    pub term_id: Uuid,
    pub section_id: Option<Uuid>,
    pub override_type: String,
    pub reason: String,
    pub expires_at: Option<DateTime<Utc>>,
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
    pub add_drop_closes_at: DateTime<Utc>,
}
