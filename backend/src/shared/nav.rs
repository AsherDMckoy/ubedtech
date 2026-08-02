//! Role-aware primary navigation — the ONE source of which nav items a
//! signed-in actor sees. Rendering only ever consumes `items()`, so a
//! link can't appear without a role granting it (the reported bug:
//! instructors saw the student links and collected 403s). Visibility
//! must keep equaling authorization; the drift-proof is
//! `enrollment::tests::nav_matches_policy_for_every_role`, which renders
//! each role's nav and then hits every nav target as that role.

use crate::shared::actor::Role;

pub struct NavItem {
    pub href: &'static str,
    pub label: &'static str,
    pub key: &'static str,
}

const fn item(href: &'static str, label: &'static str, key: &'static str) -> NavItem {
    NavItem { href, label, key }
}

/// Grouped by the role that unlocks them, in rail order.
const STUDENT: &[NavItem] = &[
    item("/ui/dashboard", "Dashboard", "dashboard"),
    item("/ui/catalog", "Catalog", "catalog"),
    item("/ui/registration", "My courses", "registration"),
    item("/ui/schedule", "Schedule", "schedule"),
    item("/ui/grades", "Grades", "grades"),
    item("/ui/history", "History", "history"),
    item("/ui/documents", "Documents", "documents"),
];

const INSTRUCTOR: &[NavItem] = &[item("/ui/instructor", "Your sections", "instructor")];

/// The registrar area an institution admin shares by policy
/// (`require_can_manage_academics` accepts either role)…
const REGISTRAR_AREA: &[NavItem] = &[
    item("/ui/registrar", "Overview", "overview"),
    item("/ui/registrar/terms", "Terms & windows", "terms"),
    item("/ui/registrar/sections", "Sections", "sections"),
    item("/ui/registrar/courses", "Courses", "courses"),
];

/// …and the part only a registrar holds: students (holds) and overrides
/// (`require_can_manage_holds` / `require_can_grant_override` —
/// deliberately no admin bypass).
const REGISTRAR_ONLY: &[NavItem] = &[
    item("/ui/registrar/students", "Students", "students"),
    item("/ui/registrar/overrides", "Overrides", "overrides"),
];

const DOCUMENT_OFFICER: &[NavItem] = &[item("/ui/admin/documents", "Document queue", "queue")];

const INSTITUTION_ADMIN: &[NavItem] = &[
    item("/ui/admin/calendar", "Calendar", "calendar"),
    item("/ui/admin/settings", "Settings", "settings"),
    item("/ui/admin/accounts", "Accounts", "accounts"),
];

const PLATFORM: &[NavItem] = &[item("/ui/platform/license", "License", "license")];

/// Nav items for the request's actor (roles come from the request scope;
/// empty when signed out). `RecordsOfficer` grants no item of its own —
/// rosters are reached through the registrar/instructor surfaces its
/// holders also carry. `InstitutionAdmin` gets the registrar area too
/// because the policy grants it (`require_can_manage_academics` accepts
/// Registrar OR InstitutionAdmin) — the nav follows the policy, never
/// the other way around.
pub fn items() -> Vec<&'static NavItem> {
    let roles = crate::shared::theme::nav_roles();
    let mut items: Vec<&'static NavItem> = Vec::new();
    for (role, group) in [
        (Role::Student, STUDENT),
        (Role::Instructor, INSTRUCTOR),
        (Role::Registrar, REGISTRAR_AREA),
        (Role::Registrar, REGISTRAR_ONLY),
        (Role::InstitutionAdmin, REGISTRAR_AREA),
        (Role::DocumentOfficer, DOCUMENT_OFFICER),
        (Role::InstitutionAdmin, INSTITUTION_ADMIN),
        (Role::PlatformLicensingAdmin, PLATFORM),
    ] {
        if roles.contains(&role) {
            for item in group {
                if !items.iter().any(|seen| seen.key == item.key) {
                    items.push(item);
                }
            }
        }
    }
    items
}
