use crate::shared::{
    actor::{Actor, Role},
    error::AppError,
};
use uuid::Uuid;

pub fn require_can_register_for(actor: &Actor, target_student_id: Uuid) -> Result<(), AppError> {
    if actor.student_id == Some(target_student_id) {
        return Ok(());
    }

    if actor.has_role(Role::Registrar) {
        return Ok(());
    }

    Err(AppError::Forbidden)
}
