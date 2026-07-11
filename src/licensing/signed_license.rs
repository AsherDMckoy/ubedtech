use crate::shared::error::AppError;
use chrono::{DateTime, Utc};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize)]
pub struct LicenseClaims {
    pub institution_id: Uuid,
    pub deployment_id: Uuid,
    pub license_serial: Uuid,
    pub valid_from: DateTime<Utc>,
    pub valid_until: DateTime<Utc>,
    pub feature_set: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SignedLicenseFile {
    pub claims: LicenseClaims,
    pub signature_hex: String,
}

pub fn verify_signed_license<'a>(
    file: &'a SignedLicenseFile,
    public_key: &VerifyingKey,
    expected_deployment_id: Uuid,
) -> Result<&'a LicenseClaims, AppError> {
    let canonical = serde_json::to_vec(&file.claims).map_err(|_| AppError::Internal)?;
    let signature_bytes = hex::decode(&file.signature_hex)
        .map_err(|_| AppError::Validation("invalid license signature encoding".into()))?;
    let signature = Signature::from_slice(&signature_bytes)
        .map_err(|_| AppError::Validation("invalid license signature".into()))?;

    public_key
        .verify(&canonical, &signature)
        .map_err(|_| AppError::InstitutionLocked)?;

    if file.claims.deployment_id != expected_deployment_id {
        return Err(AppError::InstitutionLocked);
    }

    let now = Utc::now();
    if now < file.claims.valid_from || now >= file.claims.valid_until {
        return Err(AppError::InstitutionLocked);
    }

    Ok(&file.claims)
}
