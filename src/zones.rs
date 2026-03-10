//! Zone trait hierarchy for generic zone-parameterized operations.
//!
//! Corpus and inbox are structurally identical — indexed audio files with tags
//! and signals — but historically treated as separate worlds with duplicated
//! logic. This module provides a trait hierarchy that lets computations, queries,
//! and executors be written once and parameterized by zone.
//!
//! ## Trait Hierarchy
//!
//! - `AudioZone` — base: zone has indexed audio files in the `files` table
//! - `TaggedZone: AudioZone` — zone has a tag table (corpus_tags / inbox_tags)
//! - `CanonicalTagSource: TaggedZone` — zone's tags define canonical vocabulary (corpus only)
//! - `Deployable: TaggedZone` — zone's files participate in deploy path computation (corpus only)
//! - `ExternallyMatchable: AudioZone` — zone's files can be matched against external databases
//!
//! ## Associated Signal Types
//!
//! The signal write system (`CorpusSignalStore`) dispatches on concrete signal
//! type, not zone. Both corpus and inbox signals implement `CorpusSignalStore`.
//! Zone identity lives in *which signal type you pass*. `AudioZone` carries
//! associated signal types so generic code can do `Z::FilePresenceSignal::TABLE_NAME`.

use std::collections::HashSet;

use crate::db::types::Zone;
use crate::db::write_thread::SignalWriteSender;
use crate::db::ReadOnlyDb;
use crate::meta::computations::ComputationWitness;
use crate::meta::signals::registry::TypedSignalWrite;
use crate::meta::signals::store::CorpusSignalStore;

/// Marker: this zone has indexed audio files in the `files` table.
pub trait AudioZone: Send + Sync + 'static {
    const ZONE: Zone;
    const ZONE_STR: &'static str;

    /// Signal emitted when a file is first observed on disk in this zone.
    type FilePresenceSignal: CorpusSignalStore;
    /// Signal for files on disk but not yet indexed (no audio_info).
    type UnindexedSignal: CorpusSignalStore;
    /// Signal for files that are indexed and verified healthy.
    type HealthySignal: CorpusSignalStore;

    /// Construct the file-presence signal write for this zone.
    fn file_presence_signal(inode: i64, path: String, generation: u8) -> TypedSignalWrite;
    /// Construct the unindexed signal write for this zone.
    fn unindexed_signal(inode: i64, path: String) -> TypedSignalWrite;
    /// Construct the healthy signal write for this zone.
    fn healthy_signal(inode: i64, path: String) -> TypedSignalWrite;
}

/// This zone has a tag table and supports tag queries.
pub trait TaggedZone: AudioZone {
    const TAG_TABLE: &'static str;
}

/// This zone's tags are the canonical vocabulary. Only Corpus.
pub trait CanonicalTagSource: TaggedZone {}

/// This zone's files participate in deploy path computation. Only Corpus.
pub trait Deployable: TaggedZone {}

/// This zone's files can be matched against external databases.
pub trait ExternallyMatchable: AudioZone {}

/// Behavior trait for zone signal derivation.
///
/// Encodes the zone-specific policies for the derive executor:
/// how to handle gone files, healthy-file gating, and signal GC.
/// Separate from `AudioZone` so the type system expresses which zones
/// participate in derivation vs. other operations.
pub trait DeriveZoneSignals: AudioZone {
    /// Handle an indexed file that is no longer on disk.
    ///
    /// Corpus: emit MissingFileSignal + clear stale HealthyFile.
    /// Inbox: cascade-drop all inbox state.
    fn on_file_gone(
        inode: i64,
        path: &str,
        read_only_db: &ReadOnlyDb<'_>,
        sender: &SignalWriteSender,
        witness: &ComputationWitness,
    );

    /// Whether an indexed+present file should be marked healthy.
    ///
    /// Corpus: false if OOB signals exist. Inbox: always true.
    fn should_mark_healthy(inode: i64, read_only_db: &ReadOnlyDb<'_>) -> bool;

    /// GC orphaned signals for this zone. Returns count cleared.
    ///
    /// Corpus: ~18 signal tables. Inbox: 3 tables.
    fn gc_orphaned_signals(
        read_only_db: &ReadOnlyDb<'_>,
        sender: &SignalWriteSender,
        known_inodes: &HashSet<i64>,
        witness: &ComputationWitness,
    ) -> usize;

    /// Reconcile signals for a file present on both disk and index.
    ///
    /// Corpus: clear stale MissingFile/UnindexedFile, conditionally mark healthy (OOB gating).
    /// Inbox: clear stale InboxUnindexed, unconditionally mark healthy.
    fn on_file_present(
        inode: i64,
        path: &str,
        read_only_db: &ReadOnlyDb<'_>,
        sender: &SignalWriteSender,
        witness: &ComputationWitness,
    );

    /// How to compute "known inodes" for GC purposes.
    ///
    /// Corpus: disk ∪ indexed (both matter — index-only files get MissingFile).
    /// Inbox: disk only (gone inbox files are cascade-dropped).
    fn known_inodes_for_gc(disk_set: &HashSet<i64>, indexed_set: &HashSet<i64>) -> HashSet<i64>;
}

// ============================================================================
// Concrete Zone Types
// ============================================================================

pub struct CorpusZone;
pub struct InboxZone;

// ============================================================================
// Implementations
// ============================================================================

impl AudioZone for CorpusZone {
    const ZONE: Zone = Zone::Corpus;
    const ZONE_STR: &'static str = "corpus";
    type FilePresenceSignal = crate::meta::signals::data::FileInCorpusSignal;
    type UnindexedSignal = crate::meta::signals::data::UnindexedFileSignal;
    type HealthySignal = crate::meta::signals::data::HealthyFileSignal;

    fn file_presence_signal(inode: i64, path: String, generation: u8) -> TypedSignalWrite {
        TypedSignalWrite::FileInCorpus(crate::meta::signals::data::FileInCorpusSignal {
            inode,
            path,
            generation,
        })
    }

    fn unindexed_signal(inode: i64, path: String) -> TypedSignalWrite {
        TypedSignalWrite::UnindexedFile(crate::meta::signals::data::UnindexedFileSignal {
            inode,
            path,
        })
    }

    fn healthy_signal(inode: i64, path: String) -> TypedSignalWrite {
        TypedSignalWrite::HealthyFile(crate::meta::signals::data::HealthyFileSignal {
            inode,
            path,
        })
    }
}

impl TaggedZone for CorpusZone {
    const TAG_TABLE: &'static str = "corpus_tags";
}

impl CanonicalTagSource for CorpusZone {}
impl Deployable for CorpusZone {}
impl ExternallyMatchable for CorpusZone {}

impl AudioZone for InboxZone {
    const ZONE: Zone = Zone::Inbox;
    const ZONE_STR: &'static str = "inbox";
    type FilePresenceSignal = crate::meta::signals::data::FileInInboxSignal;
    type UnindexedSignal = crate::meta::signals::data::InboxUnindexedSignal;
    type HealthySignal = crate::meta::signals::data::InboxHealthySignal;

    fn file_presence_signal(inode: i64, path: String, generation: u8) -> TypedSignalWrite {
        TypedSignalWrite::FileInInbox(crate::meta::signals::data::FileInInboxSignal {
            inode,
            path,
            generation,
        })
    }

    fn unindexed_signal(inode: i64, path: String) -> TypedSignalWrite {
        TypedSignalWrite::InboxUnindexed(crate::meta::signals::data::InboxUnindexedSignal {
            inode,
            path,
        })
    }

    fn healthy_signal(inode: i64, path: String) -> TypedSignalWrite {
        TypedSignalWrite::InboxHealthy(crate::meta::signals::data::InboxHealthySignal {
            inode,
            path,
        })
    }
}

impl TaggedZone for InboxZone {
    const TAG_TABLE: &'static str = "inbox_tags";
}

impl ExternallyMatchable for InboxZone {}
