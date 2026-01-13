//! Album Artist Flow Types

/// Phases available in the album artist resolution flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlbumArtistPhase {
    /// Resolve spelling/capitalization variants of album artist names
    Canonicalization,
    /// Unify mixed-artist albums to "Various Artists" or similar
    Collation,
    /// Bulk-fill missing album_artist tags from artist field
    Population,
}

impl AlbumArtistPhase {
    /// Get display name for this phase.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Canonicalization => "Canonicalization",
            Self::Collation => "Collation",
            Self::Population => "Population",
        }
    }

    /// Get description for this phase.
    pub fn description(&self) -> &'static str {
        match self {
            Self::Canonicalization => "Resolve spelling/capitalization variants",
            Self::Collation => "Unify mixed-artist albums to Various",
            Self::Population => "Bulk-fill missing album_artist tags",
        }
    }

    /// Get all phases in order.
    pub fn all() -> &'static [AlbumArtistPhase] {
        &[
            AlbumArtistPhase::Canonicalization,
            AlbumArtistPhase::Collation,
            AlbumArtistPhase::Population,
        ]
    }
}

/// Action returned from phase selector input handling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhaseSelectorAction {
    /// No action needed
    None,
    /// User cancelled the flow
    Cancel,
    /// User selected a single phase and wants to proceed
    Proceed(AlbumArtistPhase),
}
