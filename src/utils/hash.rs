//! Password hashing.
//!
//! The Python project stored `md5(salt + password)` in a `char(32)` column and
//! relied on the client to send the MD5 digest. That is replaced by Argon2id
//! (see `docs/MIGRATION.md`): the API now accepts the plain password over the
//! wire (always use TLS), and only the Argon2id hash is stored and compared.
//!
//! Existing MD5 passwords cannot be verified by this module; the seed data in
//! `mysql/create_database.sql` was regenerated with Argon2id hashes.

use argon2::password_hash::{
    rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString,
};
use argon2::Argon2;

use crate::exception::{ApiError, ApiResult};

/// Hash a plain text password with Argon2id.
pub fn hash_password(plain: &str) -> ApiResult<String> {
    if plain.is_empty() {
        return Err(ApiError::miss_parameters("password is empty"));
    }
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(plain.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|err| ApiError::internal(format!("failed to hash password: {err}")))
}

/// Verify a plain text password against a stored Argon2id hash.
///
/// Returns `false` for malformed hashes instead of failing, so that a corrupted
/// row cannot be used to probe password validity.
pub fn verify_password(plain: &str, stored: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(stored) else {
        tracing::error!("stored password hash is not a valid PHC string");
        return false;
    };
    Argon2::default()
        .verify_password(plain.as_bytes(), &parsed)
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_are_salted_and_verifiable() {
        let first = hash_password("admin123").unwrap();
        let second = hash_password("admin123").unwrap();
        assert_ne!(first, second, "argon2 salts must be random");
        assert!(first.starts_with("$argon2id$"));
        assert!(verify_password("admin123", &first));
        assert!(verify_password("admin123", &second));
        assert!(!verify_password("wrong", &first));
    }

    #[test]
    fn malformed_hashes_never_verify() {
        assert!(!verify_password(
            "admin123",
            "d6cfa7f078f83d22a4b10169fef95e88"
        ));
        assert!(!verify_password("admin123", ""));
    }

    #[test]
    fn empty_passwords_are_rejected() {
        assert!(hash_password("").is_err());
    }
}
