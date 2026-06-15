//! Genre source provenance discriminant.
//!
//! Typed enum — not strings — per CLAUDE.md meta module typing guidance.
//! Stored as integer in SQLite on `inode_genres.source` and
//! `unresolved_genre_observations.source`.
//!
//! The PK of `inode_genres` includes `source`, so multi-source assertions
//! (e.g. "House" confirmed by FileTagImport + Discogs + MB) remain distinct
//! rows: clearing one source's rows cannot trample another's.

use serde::{Deserialize, Serialize};

/// Provenance of a single (inode, genre_id) ledger entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum GenreSource {
    /// Imported from the file's existing GENRE tag at scan time.
    FileTagImport = 1,
    /// Extracted from MusicBrainz release metadata (tags, genres array).
    MusicBrainz = 2,
    /// Extracted from a linked Discogs release (Genres or Styles array).
    Discogs = 3,
    /// Inferred by audio-feature analysis (AudioMuse / similar).
    AudiomuseInferred = 4,
    /// Asserted directly by an operator via the genre vocabulary UI.
    Manual = 5,
    /// Derived from the `genre_implies` closure — never materialized.
    /// Reserved discriminant so query-time closures can mark synthesized rows
    /// consistently without colliding with an asserted source.
    Implied = 6,
}

impl GenreSource {
    /// Integer key for SQLite storage.
    pub fn to_key(self) -> i64 {
        self as i64
    }

    /// Map back from a stored integer. Returns `None` for unknown values
    /// (forward-compat: an older client reading a row written by a newer
    /// schema should treat it as opaque rather than crash).
    pub fn from_key(key: i64) -> Option<Self> {
        match key {
            1 => Some(Self::FileTagImport),
            2 => Some(Self::MusicBrainz),
            3 => Some(Self::Discogs),
            4 => Some(Self::AudiomuseInferred),
            5 => Some(Self::Manual),
            6 => Some(Self::Implied),
            _ => None,
        }
    }

    /// Human-readable label for logs and UI.
    pub fn name(self) -> &'static str {
        match self {
            Self::FileTagImport => "FileTagImport",
            Self::MusicBrainz => "MusicBrainz",
            Self::Discogs => "Discogs",
            Self::AudiomuseInferred => "AudiomuseInferred",
            Self::Manual => "Manual",
            Self::Implied => "Implied",
        }
    }
}

/// Kind of genre assertion. Discogs distinguishes Genres (umbrella categories)
/// from Styles (specific subgenres); other sources collapse to Genre. Stored
/// on `inode_genres.kind` and included in the PK so a single Discogs release
/// can contribute both kinds for the same (inode, genre_id, source) tuple.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum GenreKind {
    Genre = 0,
    Style = 1,
}

impl GenreKind {
    pub fn to_key(self) -> i64 {
        self as i64
    }

    pub fn from_key(key: i64) -> Option<Self> {
        match key {
            0 => Some(Self::Genre),
            1 => Some(Self::Style),
            _ => None,
        }
    }
}
