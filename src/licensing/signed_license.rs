//! Self-hosted signed licenses (format v1, frozen — ADR-10).
//!
//! A license file is a JSON envelope: `format` (must be 1), `claims_json`
//! (the exact UTF-8 JSON text of the claims that was signed), and
//! `signature_hex` (Ed25519 over the raw `claims_json` bytes). Signing the
//! carried bytes — rather than a re-serialization of parsed claims — means
//! verification can never break on serialization differences between the
//! signing tool and this crate.
//!
//! The private signing key exists only on the platform operator's side and
//! is never present in a university deployment; deployments hold only the
//! public key (`APP_LICENSE_PUBLIC_KEY`). See docs/SECURITY.md for the
//! format, signing procedure, and key-rotation guidance.

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
    pub format: u32,
    pub claims_json: String,
    pub signature_hex: String,
}

/// The deployment's license-verification public key, from configuration.
/// `None` means this deployment (typically hosted) does not accept signed
/// license imports at all.
#[derive(Clone)]
pub struct ImportVerifyingKey(pub Option<VerifyingKey>);

/// Verify a signed license file and return its claims. Every failure is a
/// `Validation` error with a fixed message — the import endpoint is already
/// behind authentication, and no input is ever echoed back.
pub fn verify_signed_license(
    file: &SignedLicenseFile,
    public_key: &VerifyingKey,
    expected_deployment_id: Uuid,
) -> Result<LicenseClaims, AppError> {
    if file.format != 1 {
        return Err(AppError::Validation(
            "unsupported license file format".into(),
        ));
    }

    let signature_bytes = hex::decode(&file.signature_hex)
        .map_err(|_| AppError::Validation("invalid license signature encoding".into()))?;
    let signature = Signature::from_slice(&signature_bytes)
        .map_err(|_| AppError::Validation("invalid license signature".into()))?;

    public_key
        .verify(file.claims_json.as_bytes(), &signature)
        .map_err(|_| AppError::Validation("license signature verification failed".into()))?;

    // Only signature-verified bytes are ever parsed as claims.
    let claims: LicenseClaims = serde_json::from_str(&file.claims_json)
        .map_err(|_| AppError::Validation("malformed license claims".into()))?;

    if claims.deployment_id != expected_deployment_id {
        return Err(AppError::Validation(
            "license is for a different deployment".into(),
        ));
    }

    let now = Utc::now();
    if now < claims.valid_from || now >= claims.valid_until {
        return Err(AppError::Validation(
            "license is outside its validity window".into(),
        ));
    }

    Ok(claims)
}
