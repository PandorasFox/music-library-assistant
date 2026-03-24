//! Message and data types for external fetch scheduling.

use crate::meta::external::ExternalSource;
pub use mm_meta::witch_types::CoverArtProgress;

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
    /// Cover art fetch progress update.
    CoverArtProgress(CoverArtProgress),
    /// Cover art fetch complete.
    CoverArtDone(CoverArtProgress),
}

/// Command from Witch to scheduler.
pub(super) enum FetchCommand {
    /// Populate AcoustID + MB queues and start fetching.
    Start,
    /// Fetch cover art from Cover Art Archive for matched releases.
    StartCoverArt,
    /// Shut down the scheduler thread.
    Shutdown,
}
