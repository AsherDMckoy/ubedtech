use crate::shared::{
    actor::{Actor, Role},
    error::AppError,
};

/// Terms, courses, sections, meetings, and prerequisites are the academic
/// structure: the registrar owns it day to day, the institution admin may
/// also act (initial setup, registrar vacancy).
pub fn require_can_manage_academics(actor: &Actor) -> Result<(), AppError> {
    if actor.has_role(Role::Registrar) || actor.has_role(Role::InstitutionAdmin) {
        return Ok(());
    }
    Err(AppError::Forbidden)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use uuid::Uuid;

    #[test]
    fn only_registrar_and_institution_admin_manage_academics() {
        for role in [
            Role::Student,
            Role::Instructor,
            Role::Registrar,
            Role::RecordsOfficer,
            Role::DocumentOfficer,
            Role::InstitutionAdmin,
            Role::PlatformLicensingAdmin,
        ] {
            let actor = Actor {
                user_id: Uuid::new_v4(),
                institution_id: Uuid::new_v4(),
                student_id: None,
                roles: HashSet::from([role]),
            };
            let allowed = require_can_manage_academics(&actor).is_ok();
            assert_eq!(
                allowed,
                matches!(role, Role::Registrar | Role::InstitutionAdmin),
                "role {role:?}"
            );
        }
    }
}
