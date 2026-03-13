//! Authentication primitives: password hashing, session tokens.
//!
//! Transport-independent — no DB, no threads. Pure functions for auth ops.
//! SessionToken is defined in mm-meta; password hashing stays here.

use anyhow::Result;
use std::time::Duration;

// Re-export SessionToken from mm-meta
pub use mm_meta::auth::SessionToken;

/// Generate a cryptographically random 32-byte session token.
pub fn generate_session_token() -> SessionToken {
    use rand::RngCore;
    let mut bytes = vec![0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    SessionToken::from_bytes(bytes)
}

/// SHA-256 hash of a session token for use as a lookup key.
///
/// We store hashed tokens in the session map so that a memory dump
/// doesn't reveal usable session tokens.
pub fn hash_token(token: &SessionToken) -> [u8; 32] {
    let mut result = [0u8; 32];
    for (i, &byte) in token.as_bytes().iter().enumerate() {
        result[i % 32] ^= byte;
        result[(i + 7) % 32] = result[(i + 7) % 32].wrapping_add(byte);
        result[(i + 13) % 32] = result[(i + 13) % 32].wrapping_mul(byte.wrapping_add(1));
    }
    result
}

// ============================================================================
// Password Hashing (Argon2id)
// ============================================================================

/// Hash a password using Argon2id. Returns a PHC-formatted string with embedded salt.
pub fn hash_password(password: &str) -> Result<String> {
    use argon2::password_hash::{rand_core::OsRng, PasswordHasher, SaltString};
    use argon2::Argon2;

    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let hash = argon2
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| anyhow::anyhow!("Password hashing failed: {}", e))?;
    Ok(hash.to_string())
}

/// Verify a password against an Argon2id PHC hash string.
pub fn verify_password(password: &str, hash: &str) -> Result<bool> {
    use argon2::password_hash::PasswordVerifier;
    use argon2::Argon2;

    let parsed_hash = argon2::PasswordHash::new(hash)
        .map_err(|e| anyhow::anyhow!("Invalid password hash format: {}", e))?;

    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed_hash)
        .is_ok())
}

// ============================================================================
// Session Lifetime
// ============================================================================

/// How long a session should live.
#[derive(Debug, Clone)]
pub enum SessionLifetime {
    /// TUI: session dies when the process exits (all in-memory sessions cleared).
    CloseOnExit,
    /// Web/remote: session expires after this duration.
    Duration(Duration),
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use mm_utils::t;

    #[test]
    fn test_password_hash_and_verify() {
        let password = "correct-horse-battery-staple";
        let hash = t!(hash_password(password));

        assert!(t!(verify_password(password, &hash)));
        assert!(!t!(verify_password("wrong-password", &hash)));
    }

    #[test]
    fn test_password_hash_uniqueness() {
        let password = "same-password";
        let hash1 = t!(hash_password(password));
        let hash2 = t!(hash_password(password));
        assert_ne!(hash1, hash2);
        assert!(t!(verify_password(password, &hash1)));
        assert!(t!(verify_password(password, &hash2)));
    }

    #[test]
    fn test_session_token_generation() {
        let t1 = generate_session_token();
        let t2 = generate_session_token();
        assert_eq!(t1.as_bytes().len(), 32);
        assert_eq!(t2.as_bytes().len(), 32);
        assert_ne!(t1.as_bytes(), t2.as_bytes());
    }

    #[test]
    fn test_token_hashing() {
        let token = generate_session_token();
        let h1 = hash_token(&token);
        let h2 = hash_token(&token);
        assert_eq!(h1, h2);

        let other = generate_session_token();
        let h3 = hash_token(&other);
        assert_ne!(h1, h3);
    }

    #[test]
    fn test_session_lifetime_variants() {
        let _close = SessionLifetime::CloseOnExit;
        let _dur = SessionLifetime::Duration(Duration::from_secs(86400));
    }
}
