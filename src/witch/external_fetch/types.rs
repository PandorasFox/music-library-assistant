//! Message and data types for external fetch scheduling.

use crate::meta::external::ExternalSource;

// ============================================================================
// Public Types
// ============================================================================

/// A match row to write to external_matches (recording MBID + confidence only).
pub struct MatchRow {
    pub recording_id: String,
    pub confidence: f64,
}

impl std::fmt::Debug for MatchRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MatchRow")
            .field("recording_id", &self.recording_id)
            .field("confidence", &self.confidence)
            .finish()
    }
}

// Protocol-visible progress types — re-exported from mm-meta.
pub use mm_meta::witch_types::{FetchProgress, SourceProgress};

/// What kind of MusicBrainz entity to fetch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MbEntityKind {
    Recording,
    Artist,
    Release,
}

impl MbEntityKind {
    /// The entity_type string used in mb_known_entities table.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Recording => "recording",
            Self::Artist => "artist",
            Self::Release => "release",
        }
    }
}

// ============================================================================
// Internal Types (Scheduler <-> Witch)
// ============================================================================

/// Message from scheduler to Witch (status updates only).
pub(in crate::witch) enum SchedulerMessage {
    /// Intermediate progress snapshot.
    Progress(FetchProgress),
    /// One source's queue has drained.
    SourceDone {
        source: ExternalSource,
        stats: SourceProgress,
    },
    /// Both sources done -- scheduler going back to sleep.
    AllDone,
}

/// Command from Witch to scheduler.
pub(super) enum FetchCommand {
    /// Scan eligible dirs for inodes needing AcoustID fingerprint lookup.
    /// Also populates MB queue for existing matches needing enrichment.
    Start { eligible_dirs: Vec<std::path::PathBuf> },
    /// Shut down the scheduler thread.
    Shutdown,
}
