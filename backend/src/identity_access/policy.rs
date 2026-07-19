//! Identity-domain authorization decisions, pure and unit-testable.
//!
//! Feature-domain policies live with their features (`enrollment::policy`,
//! grade rules in `records`, document rules in `documents`, license rules in
//! `licensing`). This module owns the decisions about accounts, credentials,
//! and role assignment themselves.

use crate::shared::actor::{Actor, Role};
use crate::shared::error::AppError;

/// Suspending accounts and resetting other people's passwords is
/// institution-admin work.
pub fn require_can_manage_accounts(actor: &Actor) -> Result<(), AppError> {
    if actor.has_role(Role::InstitutionAdmin) {
        return Ok(());
    }
    Err(AppError::Forbidden)
}

/// Granting and revoking roles is institution-admin work.
pub fn require_can_manage_roles(actor: &Actor) -> Result<(), AppError> {
    if actor.has_role(Role::InstitutionAdmin) {
        return Ok(());
    }
    Err(AppError::Forbidden)
}

/// Roles an institution admin may grant or revoke.
///
/// The platform licensing role belongs to the platform operator, not the
/// institution: no institution-side actor may hand it out or take it away.
/// It is managed only by the operator bootstrap procedure (fail closed).
pub fn assignable_by_institution_admin(role: Role) -> bool {
    !matches!(role, Role::PlatformLicensingAdmin)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use uuid::Uuid;

    const ALL_ROLES: [Role; 7] = [
        Role::Student,
        Role::Instructor,
        Role::Registrar,
        Role::RecordsOfficer,
        Role::DocumentOfficer,
        Role::InstitutionAdmin,
        Role::PlatformLicensingAdmin,
    ];

    fn actor_with(role: Role) -> Actor {
        Actor {
            user_id: Uuid::new_v4(),
            institution_id: Uuid::new_v4(),
            student_id: None,
            roles: HashSet::from([role]),
        }
    }

    #[test]
    fn only_institution_admin_manages_accounts_and_roles() {
        for role in ALL_ROLES {
            let actor = actor_with(role);
            let expected = role == Role::InstitutionAdmin;
            assert_eq!(
                require_can_manage_accounts(&actor).is_ok(),
                expected,
                "accounts: {role:?}"
            );
            assert_eq!(
                require_can_manage_roles(&actor).is_ok(),
                expected,
                "roles: {role:?}"
            );
        }
    }

    #[test]
    fn an_actor_with_no_roles_manages_nothing() {
        let actor = Actor {
            user_id: Uuid::new_v4(),
            institution_id: Uuid::new_v4(),
            student_id: None,
            roles: HashSet::new(),
        };
        assert!(require_can_manage_accounts(&actor).is_err());
        assert!(require_can_manage_roles(&actor).is_err());
    }

    #[test]
    fn the_platform_role_is_never_institution_assignable() {
        for role in ALL_ROLES {
            assert_eq!(
                assignable_by_institution_admin(role),
                role != Role::PlatformLicensingAdmin,
                "{role:?}"
            );
        }
    }
}
