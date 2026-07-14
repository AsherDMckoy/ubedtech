use std::collections::HashSet;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    Student,
    Instructor,
    Registrar,
    RecordsOfficer,
    DocumentOfficer,
    InstitutionAdmin,
    PlatformLicensingAdmin,
}

impl Role {
    /// Database code (`role.code`) for this role, seeded by migration 0007.
    pub fn code(self) -> &'static str {
        match self {
            Role::Student => "student",
            Role::Instructor => "instructor",
            Role::Registrar => "registrar",
            Role::RecordsOfficer => "records_officer",
            Role::DocumentOfficer => "document_officer",
            Role::InstitutionAdmin => "institution_admin",
            Role::PlatformLicensingAdmin => "platform_licensing_admin",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Some(match code {
            "student" => Role::Student,
            "instructor" => Role::Instructor,
            "registrar" => Role::Registrar,
            "records_officer" => Role::RecordsOfficer,
            "document_officer" => Role::DocumentOfficer,
            "institution_admin" => Role::InstitutionAdmin,
            "platform_licensing_admin" => Role::PlatformLicensingAdmin,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone)]
pub struct Actor {
    pub user_id: Uuid,
    pub institution_id: Uuid,
    pub student_id: Option<Uuid>,
    pub roles: HashSet<Role>,
}

impl Actor {
    pub fn has_role(&self, role: Role) -> bool {
        self.roles.contains(&role)
    }

    pub fn require_student_self(&self) -> Result<Uuid, crate::shared::error::AppError> {
        self.student_id
            .ok_or(crate::shared::error::AppError::Forbidden)
    }
}
