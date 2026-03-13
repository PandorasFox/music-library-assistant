//! Message and data types for external fetch scheduling.

use std::path::PathBuf;

use crate::meta::external::ExternalSource;

// ============================================================================
// Public Types
// ============================================================================

/// A single external API call to execute on rayon.
#[derive(Debug, Clone)]
pub enum ExternalFetchTask {
    AcoustId(AcoustIdFetchTask),
    MusicBrainz(MbFetchTask),
}

/// AcoustID fingerprint lookup task.
#[derive(Debug, Clone)]
pub struct AcoustIdFetchTask {
    pub inode: i64,
    pub fingerprint_raw: Vec<u32>,
    pub fingerprint_blob: Vec<u8>,
    pub duration_secs: u32,
    pub api_key: String,
}

/// MusicBrainz entity fetch task.
#[derive(Debug, Clone)]
pub struct MbFetchTask {
    pub kind: MbEntityKind,
    pub mbid: String,
    pub base_url: String,
}

impl ExternalFetchTask {
    /// Human-readable label for status display.
    pub fn label(&self) -> &str {
        match self {
            Self::AcoustId(_) => "AcoustID lookup",
            Self::MusicBrainz(t) => match t.kind {
                MbEntityKind::Recording => "MB recording fetch",
                MbEntityKind::Artist => "MB artist fetch",
                MbEntityKind::Release => "MB release fetch",
            },
        }
    }
}

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

/// Message from scheduler to Witch (tasks + status updates).
pub(in crate::witch) enum SchedulerMessage {
    /// A task for the Witch to execute on rayon.
    TaskRequest {
        task: ExternalFetchTask,
        label: String,
    },
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

/// Result of an external fetch task execution.
///
/// Used both as the rayon task result (carried in `TaskResult::fetch_result`)
/// and as the outcome sent from Witch back to scheduler for chain-emit and
/// progress tracking. DB writes already happened on the rayon thread.
#[derive(Debug)]
pub enum FetchOutcome {
    AcoustIdMatch {
        recordings: Vec<MatchRow>,
    },
    AcoustIdNoMatch,
    AcoustIdRateLimited {
        task: ExternalFetchTask,
    },
    AcoustIdError,
    /// MB entity fetched successfully. `discovered_entities` carries newly
    /// discovered artist/release IDs (from recording parsing) for the
    /// scheduler to queue as follow-up fetches.
    MbFound {
        discovered_entities: Vec<(MbEntityKind, String)>,
    },
    MbNotFound,
    MbRateLimited {
        task: ExternalFetchTask,
    },
    MbError,
}

/// Command from Witch to scheduler.
pub(super) enum FetchCommand {
    /// Scan eligible dirs for inodes needing AcoustID fingerprint lookup.
    /// Also populates MB queue for existing matches needing enrichment.
    Start { eligible_dirs: Vec<PathBuf> },
    /// Shut down the scheduler thread.
    Shutdown,
}
