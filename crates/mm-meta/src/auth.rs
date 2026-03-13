//! Session token type for authentication.
//!
//! Transport-independent — just the token type itself.
//! Password hashing is a server-side implementation detail (auth thread).

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

/// Proof that code is executing in the first-time setup path.
///
/// Zero-sized witness type. The Witch and first-time setup code create this
/// to prove setup context to the database creation function.
pub struct FirstTimeSetupToken(());

impl FirstTimeSetupToken {
    /// Create a setup token. Only the Witch and first-time setup code should call this.
    pub fn new() -> Self {
        Self(())
    }
}
