use std::sync::Arc;

use crate::shared::error::AppError;
use arc_swap::ArcSwap;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LicenseSnapshot {
    pub institution_id: Uuid,
    pub deployment_id: Uuid,
    pub status: LicenseStatus,
    pub valid_from: DateTime<Utc>,
    pub valid_until: DateTime<Utc>,
    pub version: i64,
    pub feature_set: serde_json::Value,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LicenseStatus {
    Active,
    Suspended,
    Expired,
}

#[derive(Clone)]
pub struct LicenseGate {
    current: Arc<ArcSwap<LicenseSnapshot>>,
}

impl LicenseGate {
    pub fn new(snapshot: LicenseSnapshot) -> Self {
        Self {
            current: Arc::new(ArcSwap::from_pointee(snapshot)),
        }
    }

    pub fn require_active(&self, institution_id: Uuid) -> Result<(), AppError> {
        let snapshot = self.current.load();
        let now = Utc::now();

        let active = snapshot.institution_id == institution_id
            && snapshot.status == LicenseStatus::Active
            && now >= snapshot.valid_from
            && now < snapshot.valid_until;

        if active {
            Ok(())
        } else {
            Err(AppError::InstitutionLocked)
        }
    }

    pub fn replace(&self, snapshot: LicenseSnapshot) {
        self.current.store(Arc::new(snapshot));
    }

    pub fn snapshot(&self) -> Arc<LicenseSnapshot> {
        self.current.load_full()
    }
}
