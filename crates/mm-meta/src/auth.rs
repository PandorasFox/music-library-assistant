//! Session token type for authentication.
//!
//! Transport-independent — just the token type itself.
//! Password hashing and session management stay in the mm crate.

/// Opaque session token (32 random bytes). Client stores this.
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
