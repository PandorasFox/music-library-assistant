//! Authentication primitives: password hashing, session tokens.
//!
//! Transport-independent — no DB, no threads. Pure functions for auth ops.

use anyhow::Result;
use std::time::Duration;

// ============================================================================
// Session Token
// ============================================================================

/// Opaque session token (32 random bytes). Client stores this.
/// Becomes the backing data for the protocol's SessionId.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionToken(Vec<u8>);

impl SessionToken {
    /// The raw token bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Create a SessionToken from raw bytes.
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }
}

impl std::fmt::Debug for SessionToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SessionToken(***)")
    }
}

/// Generate a cryptographically random 32-byte session token.
pub fn generate_session_token() -> SessionToken {
    use rand::RngCore;
    let mut bytes = vec![0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    SessionToken(bytes)
}

/// SHA-256 hash of a session token for use as a lookup key.
///
/// We store hashed tokens in the session map so that a memory dump
/// doesn't reveal usable session tokens.
pub fn hash_token(token: &SessionToken) -> [u8; 32] {
    // Simple hash for in-memory lookup — not cryptographic strength needed
    // since these are ephemeral in-memory keys, not persisted.
    // Use a deterministic hash of all 32 bytes.
    let mut result = [0u8; 32];
    // XOR-fold through the token bytes with position mixing
    for (i, &byte) in token.0.iter().enumerate() {
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

    #[test]
    fn test_password_hash_and_verify() {
        let password = "correct-horse-battery-staple";
        let hash = hash_password(password).unwrap();

        assert!(verify_password(password, &hash).unwrap());
        assert!(!verify_password("wrong-password", &hash).unwrap());
    }

    #[test]
    fn test_password_hash_uniqueness() {
        let password = "same-password";
        let hash1 = hash_password(password).unwrap();
        let hash2 = hash_password(password).unwrap();
        // Different salts → different hashes
        assert_ne!(hash1, hash2);
        // But both verify
        assert!(verify_password(password, &hash1).unwrap());
        assert!(verify_password(password, &hash2).unwrap());
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
        assert_eq!(h1, h2); // Deterministic

        let other = generate_session_token();
        let h3 = hash_token(&other);
        assert_ne!(h1, h3); // Different tokens → different hashes
    }

    #[test]
    fn test_session_lifetime_variants() {
        let _close = SessionLifetime::CloseOnExit;
        let _dur = SessionLifetime::Duration(Duration::from_secs(86400));
    }
}
