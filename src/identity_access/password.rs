//! Argon2id password hashing and verification.
//!
//! Parameters come from configuration (`APP_ARGON2_MEMORY_KIB`,
//! `APP_ARGON2_TIME_COST`, `APP_ARGON2_PARALLELISM`); the defaults are the
//! OWASP Password Storage Cheat Sheet's first recommended Argon2id
//! configuration (19 MiB, 2 iterations, 1 lane). Parameters are embedded in
//! each PHC hash string, so raising them later only affects newly (re)hashed
//! passwords — old hashes still verify with the parameters they were created
//! with.
//!
//! Only Argon2id hashes are accepted at verification time. Anything else in
//! `password_credential.password_hash` (a legacy scheme, a corrupted value)
//! is an error, never a silent pass or a downgrade.

use argon2::password_hash::{PasswordHash, PasswordHasher as _, PasswordVerifier as _, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use rand::TryRngCore;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PasswordError {
    #[error("invalid Argon2 parameters")]
    InvalidParams,
    #[error("password hashing failed")]
    Hashing,
    #[error("stored hash is not a supported argon2id hash")]
    UnsupportedHash,
}

#[derive(Clone)]
pub struct PasswordService {
    argon2: Argon2<'static>,
}

impl PasswordService {
    #[allow(dead_code)] // constructed by login (slice 2.3) and bootstrap (slice 2.8) this session
    pub fn new(memory_kib: u32, time_cost: u32, parallelism: u32) -> Result<Self, PasswordError> {
        let params = Params::new(memory_kib, time_cost, parallelism, None)
            .map_err(|_| PasswordError::InvalidParams)?;
        Ok(Self {
            argon2: Argon2::new(Algorithm::Argon2id, Version::V0x13, params),
        })
    }

    #[allow(dead_code)] // constructed by login (slice 2.3) and bootstrap (slice 2.8) this session
    pub fn from_config(config: &crate::config::AppConfig) -> Result<Self, PasswordError> {
        Self::new(
            config.argon2_memory_kib,
            config.argon2_time_cost,
            config.argon2_parallelism,
        )
    }

    /// Hash a password into a PHC string (salt and parameters embedded).
    ///
    /// Memory- and CPU-hard on purpose: HTTP call sites must run this via
    /// `web::block`, never directly on an Actix worker thread (CLAUDE.md §4).
    #[allow(dead_code)] // called by login (slice 2.3) and bootstrap (slice 2.8) this session
    pub fn hash(&self, password: &str) -> Result<String, PasswordError> {
        // OS entropy for the salt; RECOMMENDED_LENGTH is 16 bytes.
        let mut salt_bytes = [0u8; argon2::RECOMMENDED_SALT_LEN];
        rand::rngs::OsRng
            .try_fill_bytes(&mut salt_bytes)
            .map_err(|_| PasswordError::Hashing)?;
        let salt = SaltString::encode_b64(&salt_bytes).map_err(|_| PasswordError::Hashing)?;

        self.argon2
            .hash_password(password.as_bytes(), &salt)
            .map(|hash| hash.to_string())
            .map_err(|_| PasswordError::Hashing)
    }

    /// Verify a candidate against a stored PHC string.
    ///
    /// `Ok(false)` means "wrong password"; `Err(UnsupportedHash)` means the
    /// stored value itself is unusable (not argon2id / malformed) — callers
    /// must treat that as a server-side fault to log, while still answering
    /// the client with the same generic authentication failure. Same
    /// `web::block` requirement as `hash`.
    #[allow(dead_code)] // called by login (slice 2.3) this session
    pub fn verify(&self, stored_phc: &str, candidate: &str) -> Result<bool, PasswordError> {
        let parsed = PasswordHash::new(stored_phc).map_err(|_| PasswordError::UnsupportedHash)?;

        if parsed.algorithm != Algorithm::Argon2id.ident() {
            return Err(PasswordError::UnsupportedHash);
        }

        // verify_password compares digests in constant time internally.
        match self.argon2.verify_password(candidate.as_bytes(), &parsed) {
            Ok(()) => Ok(true),
            Err(argon2::password_hash::Error::Password) => Ok(false),
            Err(_) => Err(PasswordError::UnsupportedHash),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimum-cost parameters keep unit tests fast; the production defaults
    /// are asserted by the config tests.
    fn fast() -> PasswordService {
        PasswordService::new(8, 1, 1).unwrap()
    }

    #[test]
    fn hash_verify_roundtrip() {
        let service = fast();
        let hash = service.hash("correct horse battery staple").unwrap();
        assert!(
            service
                .verify(&hash, "correct horse battery staple")
                .unwrap()
        );
    }

    #[test]
    fn wrong_password_fails_verification() {
        let service = fast();
        let hash = service.hash("right").unwrap();
        assert_eq!(service.verify(&hash, "wrong"), Ok(false));
    }

    #[test]
    fn each_hash_gets_a_fresh_salt() {
        let service = fast();
        let a = service.hash("same password").unwrap();
        let b = service.hash("same password").unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn configured_parameters_are_embedded_in_the_hash() {
        let service = PasswordService::new(1024, 3, 2).unwrap();
        let hash = service.hash("p").unwrap();
        assert!(
            hash.starts_with("$argon2id$"),
            "unexpected PHC prefix: {hash}"
        );
        assert!(hash.contains("m=1024,t=3,p=2"), "params missing: {hash}");
    }

    #[test]
    fn non_argon2id_hashes_are_rejected_not_verified() {
        let service = fast();

        // A valid argon2i (not id) hash of the same password.
        let argon2i = Argon2::new(
            Algorithm::Argon2i,
            Version::V0x13,
            Params::new(8, 1, 1, None).unwrap(),
        );
        let legacy = argon2i
            .hash_password(
                b"password",
                &SaltString::encode_b64(b"0123456789abcdef").unwrap(),
            )
            .unwrap()
            .to_string();

        assert_eq!(
            service.verify(&legacy, "password"),
            Err(PasswordError::UnsupportedHash)
        );
        assert_eq!(
            service.verify("not-a-phc-string", "password"),
            Err(PasswordError::UnsupportedHash)
        );
        assert_eq!(
            service.verify("", "password"),
            Err(PasswordError::UnsupportedHash)
        );
    }

    #[test]
    fn invalid_parameters_are_rejected_at_construction() {
        // Argon2 requires memory >= 8 KiB per lane; zero of anything is invalid.
        assert!(matches!(
            PasswordService::new(0, 0, 0),
            Err(PasswordError::InvalidParams)
        ));
        assert!(matches!(
            PasswordService::new(8, 1, 0),
            Err(PasswordError::InvalidParams)
        ));
    }
}
