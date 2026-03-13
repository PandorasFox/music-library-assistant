//! Signal registry — single source of truth for signal type enumeration.
//!
//! The `signal_registry!` macro generates:
//! - `TypedSignalWrite` enum with all signal variants
//! - `insert()`, `exists()` dispatch on that enum
//! - `count_signal_type()` function for health queries
//! - `signal_table_entries()` for schema inventory

use crate::db::table_schema::{TableEntry, TableKind};

/// Content hash trait for change detection on typed signals.
///
/// Each signal type implements this to hash its non-PK fields.
/// BLOB signals hash serialized bytes; scalar signals hash field values.
pub trait SignalContentHash {
    fn content_hash_fields(&self, hasher: &mut std::hash::DefaultHasher);
}

/// Declares the complete signal registry.
///
/// Generates `TypedSignalWrite`, dispatch methods, `count_signal_type()`,
/// and `signal_table_entries()` from a single declaration.
macro_rules! signal_registry {
    (
        corpus {
            $($c_variant:ident($c_type:ty, $c_slug:literal)),* $(,)?
        }
        aggregate {
            $($a_variant:ident($a_type:ty, $a_slug:literal)),* $(,)?
        }
    ) => {
        /// Typed signal data for direct writes to per-signal tables.
        ///
        /// Sent through the db_thread channel to avoid JSON serialization.
        /// Each variant wraps the typed signal data struct and maps 1:1 to a table.
        #[derive(Debug, Clone)]
        pub enum TypedSignalWrite {
            $($c_variant($c_type),)*
            $($a_variant($a_type),)*
        }

        impl TypedSignalWrite {
            /// Insert this signal into its typed table.
            pub fn insert(self, conn: &rusqlite::Connection) -> rusqlite::Result<()> {
                use crate::meta::signals::store::{AggregateSignalStore, CorpusSignalStore};
                match self {
                    $(Self::$c_variant(s) => s.insert(conn),)*
                    $(Self::$a_variant(s) => s.insert(conn),)*
                }
            }

            /// Check if this signal already exists in its typed table.
            pub fn exists(&self, conn: &rusqlite::Connection) -> bool {
                use crate::meta::signals::store::{AggregateSignalStore, CorpusSignalStore};
                let result = match self {
                    $(Self::$c_variant(s) => <$c_type>::exists(conn, s.inode),)*
                    $(Self::$a_variant(s) => <$a_type>::exists(conn, &s.key),)*
                };
                result.unwrap_or(false)
            }

            /// Compute a content hash for change detection.
            ///
            /// Includes the enum discriminant for type safety.
            pub fn content_hash(&self) -> u64 {
                use std::hash::{Hash, Hasher};
                let mut hasher = std::hash::DefaultHasher::new();
                std::mem::discriminant(self).hash(&mut hasher);
                match self {
                    $(Self::$c_variant(s) => s.content_hash_fields(&mut hasher),)*
                    $(Self::$a_variant(s) => s.content_hash_fields(&mut hasher),)*
                }
                hasher.finish()
            }
        }

        /// Count signals of a specific type using typed tables.
        ///
        /// Maps short slug names to concrete signal types.
        pub fn count_signal_type(
            conn: &rusqlite::Connection,
            signal_type: &str,
        ) -> rusqlite::Result<usize> {
            use crate::meta::signals::store::{AggregateSignalStore, CorpusSignalStore};
            let count = match signal_type {
                $($c_slug => <$c_type>::count(conn)?,)*
                $($a_slug => <$a_type>::count(conn)?,)*
                _ => 0,
            };
            Ok(count)
        }

        /// Returns all signal table entries for the schema inventory.
        ///
        /// Decision signals (canonical_tag, expected_overlap, expected_duplicate,
        /// expected_missing_tag) use `TableKind::Decision`; all others use `Computed`.
        pub fn signal_table_entries() -> Vec<TableEntry> {
            use crate::meta::signals::store::{AggregateSignalStore, CorpusSignalStore};
            let mut entries = Vec::new();
            $(
                entries.push(TableEntry {
                    name: <$c_type as CorpusSignalStore>::TABLE_NAME,
                    kind: signal_registry!(@table_kind $c_slug),
                    create_sql: <$c_type as CorpusSignalStore>::TABLE_SQL,
                    index_sql: &[],
                });
            )*
            $(
                entries.push(TableEntry {
                    name: <$a_type as AggregateSignalStore>::TABLE_NAME,
                    kind: signal_registry!(@table_kind $a_slug),
                    create_sql: <$a_type as AggregateSignalStore>::TABLE_SQL,
                    index_sql: &[],
                });
            )*
            entries
        }

    };

    // --- Internal helpers ---

    // Decision tables: canonical_tag, expected_overlap, expected_duplicate, expected_missing_tag
    (@table_kind "canonical_tag") => { TableKind::Decision };
    (@table_kind "expected_overlap") => { TableKind::Decision };
    (@table_kind "expected_duplicate") => { TableKind::Decision };
    (@table_kind "expected_missing_tag") => { TableKind::Decision };
    (@table_kind $slug:literal) => { TableKind::Computed };
}

// ============================================================================
// Signal Registry Declaration
// ============================================================================

use super::data::*;

signal_registry! {
    corpus {
        FileInCorpus(FileInCorpusSignal, "file_in_corpus"),
        UnindexedFile(UnindexedFileSignal, "unindexed_file"),
        HealthyFile(HealthyFileSignal, "healthy_file"),
        FileInInbox(FileInInboxSignal, "file_in_inbox"),
        InboxUnindexed(InboxUnindexedSignal, "inbox_unindexed"),
        InboxHealthy(InboxHealthySignal, "inbox_healthy"),
        InboxCorpusMatch(InboxCorpusMatchSignal, "inbox_corpus_match"),
        CorruptFile(CorruptFileSignal, "corrupt_file"),
        MtimeOnlyMismatch(MtimeOnlyMismatchSignal, "mtime_only_mismatch"),
        MissingDirectory(MissingDirectorySignal, "missing_directory"),
        MissingFile(MissingFileSignal, "missing_file"),
        MovedFile(MovedFileSignal, "moved_file"),
        ShitFormat(ShitFormatSignal, "shit_format"),
        DeployReady(DeployReadySignal, "deploy_ready"),
        DeployedHealthy(DeployedHealthySignal, "deployed_healthy"),
        SidecarDeployReady(SidecarDeployReadySignal, "sidecar_deploy_ready"),
        OutOfBandTagSync(OutOfBandTagSyncSignal, "oob_tag_sync"),
        OutOfBandTagConflict(OutOfBandTagConflictSignal, "oob_tag_conflict"),
        SubparDuplicate(SubparDuplicateSignal, "subpar_duplicate"),
        CompoundTag(CompoundTagSignal, "compound_tag"),
        PathTagMismatch(PathTagMismatchSignal, "path_tag_mismatch"),
        ExternalMatch(ExternalMatchSignal, "external_match"),
        ReleasePacking(ReleasePackingSignal, "release_packing"),
        UnmatchedCorpusTrack(UnmatchedCorpusTrackSignal, "unmatched_corpus_track"),
        ExpectedMissingTag(ExpectedMissingTagSignal, "expected_missing_tag"),
        InboxCompoundTag(InboxCompoundTagSignal, "inbox_compound_tag"),
    }
    aggregate {
        CanonicalTag(CanonicalTagSignal, "canonical_tag"),
        ExpectedOverlap(ExpectedOverlapSignal, "expected_overlap"),
        ExpectedDuplicate(ExpectedDuplicateSignal, "expected_duplicate"),
        LibraryLeftover(LibraryLeftoverSignal, "library_leftover"),
        LibraryStale(LibraryStaleSignal, "library_stale"),
        FingerprintOverlap(FingerprintOverlapSignal, "fingerprint_dup"),
        MetadataDuplicate(MetadataDuplicateSignal, "metadata_dup"),
        DuplicateInode(DuplicateInodeSignal, "duplicate_inode"),
        MissingTag(MissingTagSignal, "missing_tag"),
        MissingAlbumSingle(MissingAlbumSingleSignal, "missing_album_single"),
        DeployConflict(DeployConflictSignal, "deploy_conflict"),
        SidecarDeployConflict(SidecarDeployConflictSignal, "sidecar_deploy_conflict"),
        TagCanonicity(TagCanonicitySignal, "tag_canonicity"),
        InconsistentAlbumArtist(InconsistentAlbumArtistSignal, "inconsistent_album_artist"),
        CrossSourceOverlap(CrossSourceOverlapSignal, "cross_source_overlap"),
        ReleaseOverlap(ReleaseOverlapSignal, "release_overlap"),
        RedundantDuplicate(RedundantDuplicateSignal, "redundant_duplicate"),
        InboxTagCanonicity(InboxTagCanonicitySignal, "inbox_tag_canonicity"),
        InboxMissingTag(InboxMissingTagSignal, "inbox_missing_tag"),
        DiscExtraction(DiscExtractionSignal, "disc_extraction"),
        UnfilledReleaseSlot(UnfilledReleaseSlotSignal, "unfilled_release_slot"),
        PackedRelease(PackedReleaseSignal, "packed_release"),
        PackingKnot(PackingKnotSignal, "packing_knot"),
        AlternativeReleasePacking(AlternativeReleasePackingSignal, "alternative_release_packing"),
        VariousArtistsOverride(VariousArtistsOverrideSignal, "various_artists_override"),
        PinnedReleaseConflict(PinnedReleaseConflictSignal, "pinned_release_conflict"),
        CrossReleaseRecording(CrossReleaseRecordingSignal, "cross_release_recording"),
    }
}
