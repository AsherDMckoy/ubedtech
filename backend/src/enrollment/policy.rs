use crate::shared::{
    actor::{Actor, Role},
    error::AppError,
};
use uuid::Uuid;

/// A student acts on their own enrollment; a registrar acts on any student's
/// (institution scoping is enforced by the queries, which resolve targets
/// inside the actor's institution only).
pub fn require_can_register_for(actor: &Actor, target_student_id: Uuid) -> Result<(), AppError> {
    if actor.student_id == Some(target_student_id) {
        return Ok(());
    }

    if actor.has_role(Role::Registrar) {
        return Ok(());
    }

    Err(AppError::Forbidden)
}

/// Overrides lift academic-integrity rules (holds, prerequisites, capacity,
/// deadlines); only the registrar — the enrollment authority — grants them.
pub fn require_can_grant_override(actor: &Actor) -> Result<(), AppError> {
    if actor.has_role(Role::Registrar) {
        return Ok(());
    }
    Err(AppError::Forbidden)
}

/// Holds block registration; placing and releasing them is the same
/// authority as overriding them.
pub fn require_can_manage_holds(actor: &Actor) -> Result<(), AppError> {
    require_can_grant_override(actor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    const ALL_ROLES: [Role; 7] = [
        Role::Student,
        Role::Instructor,
        Role::Registrar,
        Role::RecordsOfficer,
        Role::DocumentOfficer,
        Role::InstitutionAdmin,
        Role::PlatformLicensingAdmin,
    ];

    fn actor_with(role: Role, student_id: Option<Uuid>) -> Actor {
        Actor {
            user_id: Uuid::new_v4(),
            institution_id: Uuid::new_v4(),
            student_id,
            roles: HashSet::from([role]),
        }
    }

    #[test]
    fn only_the_registrar_registers_other_students() {
        let target = Uuid::new_v4();
        for role in ALL_ROLES {
            let allowed = require_can_register_for(&actor_with(role, None), target).is_ok();
            assert_eq!(
                allowed,
                role == Role::Registrar,
                "role {role:?} registering for another student"
            );
        }
    }

    #[test]
    fn a_student_registers_only_for_themselves() {
        let own_student_id = Uuid::new_v4();
        let actor = actor_with(Role::Student, Some(own_student_id));
        assert!(require_can_register_for(&actor, own_student_id).is_ok());
        assert!(matches!(
            require_can_register_for(&actor, Uuid::new_v4()),
            Err(AppError::Forbidden)
        ));
    }

    #[test]
    fn only_the_registrar_grants_overrides() {
        for role in ALL_ROLES {
            let allowed = require_can_grant_override(&actor_with(role, None)).is_ok();
            assert_eq!(allowed, role == Role::Registrar, "role {role:?}");
        }
        // Being a student on top of any role never adds the power.
        assert!(matches!(
            require_can_grant_override(&actor_with(Role::Student, Some(Uuid::new_v4()))),
            Err(AppError::Forbidden)
        ));
    }
}
