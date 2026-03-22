//! Zone trait hierarchy for generic zone-parameterized operations.
//!
//! ## Trait Hierarchy
//!
//! - `AudioZone` — base: zone has indexed audio files in the `files` table
//! - `TaggedZone: AudioZone` — zone has a tag table (corpus_tags)
//! - `CanonicalTagSource: TaggedZone` — zone's tags define canonical vocabulary (corpus only)
//! - `Deployable: TaggedZone` — zone's files participate in deploy path computation (corpus only)
//! - `ExternallyMatchable: AudioZone` — zone's files can be matched against external databases

use std::collections::HashSet;

use crate::db::types::Zone;
use crate::db::write_thread::SignalWriteSender;
use crate::db::ReadOnlyDb;
use crate::meta::computations::ComputationWitness;
use crate::meta::signals::registry::TypedSignalWrite;
use crate::meta::signals::store::{AggregateSignalStore, CorpusSignalStore};

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

    /// Per-inode compound tag detection signal for this zone.
    type CompoundTagSignal: CorpusSignalStore;
    /// Aggregate missing tag signal for this zone.
    type MissingTagSignal: AggregateSignalStore;

    /// Construct the compound tag signal write for this zone.
    fn compound_tag_signal(
        inode: i64,
        path: String,
        compounds: Vec<crate::meta::signals::data::CompoundTagEntry>,
    ) -> TypedSignalWrite;

    /// Construct the missing tag signal write for this zone.
    fn missing_tag_signal(
        key: String,
        data: crate::meta::signals::data::MissingTagData,
    ) -> TypedSignalWrite;
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
    /// Emit MissingFileSignal + clear stale HealthyFile.
    fn on_file_gone(
        inode: i64,
        path: &str,
        read_only_db: &ReadOnlyDb<'_>,
        sender: &SignalWriteSender,
        witness: &ComputationWitness,
    );

    /// Whether an indexed+present file should be marked healthy.
    ///
    /// False if OOB signals exist.
    fn should_mark_healthy(inode: i64, read_only_db: &ReadOnlyDb<'_>) -> bool;

    /// GC orphaned signals for this zone. Returns count cleared.
    fn gc_orphaned_signals(
        read_only_db: &ReadOnlyDb<'_>,
        sender: &SignalWriteSender,
        known_inodes: &HashSet<i64>,
        witness: &ComputationWitness,
    ) -> usize;

    /// Reconcile signals for a file present on both disk and index.
    ///
    /// Clear stale MissingFile/UnindexedFile, conditionally mark healthy (OOB gating).
    fn on_file_present(
        inode: i64,
        path: &str,
        read_only_db: &ReadOnlyDb<'_>,
        sender: &SignalWriteSender,
        witness: &ComputationWitness,
    );

    /// How to compute "known inodes" for GC purposes.
    ///
    /// disk ∪ indexed (both matter — index-only files get MissingFile).
    fn known_inodes_for_gc(disk_set: &HashSet<i64>, indexed_set: &HashSet<i64>) -> HashSet<i64>;
}

// ============================================================================
// Concrete Zone Types
// ============================================================================

pub struct CorpusZone;

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
    type CompoundTagSignal = crate::meta::signals::data::CompoundTagSignal;
    type MissingTagSignal = crate::meta::signals::data::MissingTagSignal;

    fn compound_tag_signal(
        inode: i64,
        path: String,
        compounds: Vec<crate::meta::signals::data::CompoundTagEntry>,
    ) -> TypedSignalWrite {
        TypedSignalWrite::CompoundTag(crate::meta::signals::data::CompoundTagSignal {
            inode,
            path,
            compounds,
        })
    }

    fn missing_tag_signal(
        key: String,
        data: crate::meta::signals::data::MissingTagData,
    ) -> TypedSignalWrite {
        TypedSignalWrite::MissingTag(crate::meta::signals::data::MissingTagSignal { key, data })
    }
}

impl CanonicalTagSource for CorpusZone {}
impl Deployable for CorpusZone {}
impl ExternallyMatchable for CorpusZone {}
