//! Album Flow Types
//!
//! Types for album canonicalization with EP/edition detection.

use crate::corpus::health::album_normalization::NormalizedAlbum;

/// An album variant within a canonicalization bucket.
#[derive(Debug, Clone)]
pub struct AlbumVariant {
    /// The exact album name as it appears in track metadata
    pub name: String,
    /// Number of tracks with this exact album name
    pub track_count: usize,
    /// Normalized form of the album name
    pub normalized: NormalizedAlbum,
    /// Artists found on tracks with this album name (for display)
    pub artists: Vec<String>,
    /// Directories containing tracks with this album name (for display)
    pub directories: Vec<String>,
    /// File extensions (e.g., "flac", "mp3") of tracks with this album name
    pub file_types: Vec<String>,
}

/// A bucket of album variants that should potentially be unified.
#[derive(Debug, Clone)]
pub struct AlbumBucket {
    /// Normalized base name used for grouping
    pub normalized_key: String,
    /// All variants (different album names that normalize to same base)
    pub variants: Vec<AlbumVariant>,
    /// Whether this bucket has EP/LP format variants
    pub has_format_variants: bool,
    /// Whether this bucket has edition variants (Deluxe, Remaster, etc.)
    pub has_edition_variants: bool,
}

impl AlbumBucket {
    /// Get total track count across all variants.
    pub fn total_tracks(&self) -> usize {
        self.variants.iter().map(|v| v.track_count).sum()
    }

    /// Get a description of the variant types present.
    pub fn variant_description(&self) -> String {
        let mut parts = Vec::new();
        if self.has_format_variants {
            parts.push("EP/LP variants");
        }
        if self.has_edition_variants {
            parts.push("edition variants");
        }
        if parts.is_empty() {
            "spelling variants".to_string()
        } else {
            parts.join(", ")
        }
    }
}

/// A decision made about an album bucket.
#[derive(Debug, Clone)]
pub struct AlbumDecision {
    /// The bucket this decision applies to
    pub bucket: AlbumBucket,
    /// The canonical album name chosen
    pub canonical_name: String,
    /// Variant names that will be renamed to canonical
    pub variants_to_rename: Vec<String>,
    /// Generated pending changes
    pub pending_changes: Vec<crate::corpus::db::PendingChange>,
}

impl AlbumDecision {
    /// Get count of tracks that will be modified.
    pub fn affected_track_count(&self) -> usize {
        self.pending_changes.len()
    }
}

/// Actions returned from the album cluster view.
#[derive(Debug, Clone)]
pub enum AlbumClusterAction {
    None,
    Continue,
    SessionComplete,
    ShowSessionReview,
    StatusMessage(String),
}

/// Actions returned from the album review view.
#[derive(Debug, Clone)]
pub enum AlbumReviewAction {
    None,
    Continue,
    Commit,
    Cancel,
    BackToClusterView,
}
