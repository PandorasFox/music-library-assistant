//! External metadata source types.
//!
//! Typed discriminants for external API sources (AcoustID, etc.).
//! Stored as integer in SQLite for compact, type-safe keying.

use serde::{Deserialize, Serialize};

/// Discriminant for external metadata API sources.
///
/// Typed enum — not strings — per CLAUDE.md meta module typing guidance.
/// Stored as integer in SQLite for compact, type-safe keying.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum ExternalSource {
    AcoustID = 1,
    // Future: MusicBrainz = 2, Discogs = 3
}

impl ExternalSource {
    /// Integer key for SQLite storage.
    pub fn to_key(self) -> i64 {
        self as i64
    }

    /// Reconstruct from SQLite integer key.
    pub fn from_key(key: i64) -> Option<Self> {
        match key {
            1 => Some(ExternalSource::AcoustID),
            _ => None,
        }
    }

    /// Human-readable name for logging.
    pub fn name(self) -> &'static str {
        match self {
            ExternalSource::AcoustID => "AcoustID",
        }
    }
}
