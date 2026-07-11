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
