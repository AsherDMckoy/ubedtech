//! Operator/load-test helper: print the Argon2id PHC hash for a password
//! read from stdin, using the same defaults as the application. Used to
//! seed `password_credential` rows for load testing (load/README.md).
//!
//! Usage: `echo -n 'the password' | cargo run --example hash_password`

use argon2::password_hash::{PasswordHasher as _, SaltString};
use argon2::{Argon2, Params};
use rand::TryRngCore;
use std::io::Read;

fn main() {
    let mut password = String::new();
    std::io::stdin()
        .read_to_string(&mut password)
        .expect("read password from stdin");
    let password = password.trim_end_matches('\n');

    let params = Params::new(19456, 2, 1, None).expect("valid Argon2 parameters");
    let argon2 = Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut salt_bytes = [0u8; 16];
    rand::rngs::OsRng
        .try_fill_bytes(&mut salt_bytes)
        .expect("OS randomness");
    let salt = SaltString::encode_b64(&salt_bytes).expect("valid salt");
    let hash = argon2
        .hash_password(password.as_bytes(), &salt)
        .expect("hashing succeeds");
    println!("{hash}");
}
