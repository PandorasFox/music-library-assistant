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

    /// Compute a deterministic content hash for DB storage and reconciliation.
    ///
    /// Both `insert()` (writes to `data_hash` column) and `reconcile_*_signals()`
    /// (computes expected hash) call this, ensuring hash comparison is consistent.
    fn content_hash_value(&self) -> i64 {
        use std::hash::Hasher;
        let mut hasher = std::hash::DefaultHasher::new();
        self.content_hash_fields(&mut hasher);
        hasher.finish() as i64
    }
}

/// Declares the complete signal registry.
///
/// Generates `TypedSignalWrite`, dispatch methods, `count_signal_type()`,
/// `signal_table_entries()`, and corpus signal clearing functions from a
/// single declaration.
///
/// Corpus signals are sub-categorized into two groups:
/// - `mutable`: signals that should be cleared when a file's state changes
/// - `inherent`: signals discovered from intrinsic file properties (CorruptFile, LosslessRemux)
///
/// The macro generates two clearing functions:
/// - `clear_mutable_corpus_signals(conn, inode)` — clears `mutable` signals
/// - `clear_all_corpus_signals(conn, inode)` — clears `mutable` + `inherent` signals
macro_rules! signal_registry {
    (
        corpus {
            mutable {
                $($m_variant:ident($m_type:ty, $m_slug:literal)),* $(,)?
            }
            inherent {
                $($i_variant:ident($i_type:ty, $i_slug:literal)),* $(,)?
            }
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
            $($m_variant($m_type),)*
            $($i_variant($i_type),)*
            $($a_variant($a_type),)*
        }

        impl TypedSignalWrite {
            /// Insert this signal into its typed table.
            pub fn insert(self, conn: &rusqlite::Connection) -> rusqlite::Result<()> {
                use crate::meta::signals::store::{AggregateSignalStore, CorpusSignalStore};
                match self {
                    $(Self::$m_variant(s) => s.insert(conn),)*
                    $(Self::$i_variant(s) => s.insert(conn),)*
                    $(Self::$a_variant(s) => s.insert(conn),)*
                }
            }

            /// Check if this signal already exists in its typed table.
            pub fn exists(&self, conn: &rusqlite::Connection) -> bool {
                use crate::meta::signals::store::{AggregateSignalStore, CorpusSignalStore};
                let result = match self {
                    $(Self::$m_variant(s) => <$m_type>::exists(conn, s.inode),)*
                    $(Self::$i_variant(s) => <$i_type>::exists(conn, s.inode),)*
                    $(Self::$a_variant(s) => <$a_type>::exists(conn, &s.key),)*
                };
                result.unwrap_or(false)
            }

            /// Compute a content hash for change detection.
            ///
            /// Delegates to the concrete type's `content_hash_value()` so the hash
            /// matches what `insert()` stores in the `data_hash` column. No enum
            /// discriminant is included — reconciliation is always per-signal-type,
            /// so the discriminant would only cause hash mismatches.
            pub fn content_hash(&self) -> i64 {
                match self {
                    $(Self::$m_variant(s) => s.content_hash_value(),)*
                    $(Self::$i_variant(s) => s.content_hash_value(),)*
                    $(Self::$a_variant(s) => s.content_hash_value(),)*
                }
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
                $($m_slug => <$m_type>::count(conn)?,)*
                $($i_slug => <$i_type>::count(conn)?,)*
                $($a_slug => <$a_type>::count(conn)?,)*
                _ => 0,
            };
            Ok(count)
        }

        /// Returns all signal table entries for the schema inventory.
        ///
        /// Decision signals (canonical_tag, expected_overlap, expected_duplicate,
        /// expected_missing_tag) use `TableKind::Decision`; all others use `Computed`.
        #[allow(clippy::vec_init_then_push)] // macro repetition blocks prevent vec![] syntax
        pub fn signal_table_entries() -> Vec<TableEntry> {
            use crate::meta::signals::store::{AggregateSignalStore, CorpusSignalStore};
            let mut entries = Vec::new();
            $(
                entries.push(TableEntry {
                    name: <$m_type as CorpusSignalStore>::TABLE_NAME,
                    kind: signal_registry!(@table_kind $m_slug),
                    create_sql: <$m_type as CorpusSignalStore>::TABLE_SQL,
                    index_sql: &[],
                });
            )*
            $(
                entries.push(TableEntry {
                    name: <$i_type as CorpusSignalStore>::TABLE_NAME,
                    kind: signal_registry!(@table_kind $i_slug),
                    create_sql: <$i_type as CorpusSignalStore>::TABLE_SQL,
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

        /// Returns `(table_name, blob_version)` for all signal types with BLOB data.
        ///
        /// Used by the reconciler to detect stale blob schemas at startup.
        /// Signals with `BLOB_VERSION == 0` (no blob field) are excluded.
        #[allow(clippy::vec_init_then_push)]
        pub fn signal_blob_versions() -> Vec<(&'static str, u32)> {
            use crate::meta::signals::store::{AggregateSignalStore, CorpusSignalStore};
            let mut versions = Vec::new();
            $(
                if <$m_type as CorpusSignalStore>::BLOB_VERSION > 0 {
                    versions.push((
                        <$m_type as CorpusSignalStore>::TABLE_NAME,
                        <$m_type as CorpusSignalStore>::BLOB_VERSION,
                    ));
                }
            )*
            $(
                if <$i_type as CorpusSignalStore>::BLOB_VERSION > 0 {
                    versions.push((
                        <$i_type as CorpusSignalStore>::TABLE_NAME,
                        <$i_type as CorpusSignalStore>::BLOB_VERSION,
                    ));
                }
            )*
            $(
                if <$a_type as AggregateSignalStore>::BLOB_VERSION > 0 {
                    versions.push((
                        <$a_type as AggregateSignalStore>::TABLE_NAME,
                        <$a_type as AggregateSignalStore>::BLOB_VERSION,
                    ));
                }
            )*
            versions
        }

        /// Clear mutable corpus signals for an inode.
        ///
        /// Clears signals that represent mutable file state (tags, paths, deploy status).
        /// Does NOT clear inherent signals (CorruptFile, LosslessRemux) which represent
        /// intrinsic file properties discovered during indexing.
        pub fn clear_mutable_corpus_signals(conn: &rusqlite::Connection, inode: i64) {
            use crate::meta::signals::store::CorpusSignalStore;
            $(let _ = <$m_type>::clear_by_inode(conn, inode);)*
        }

        /// Clear all corpus signals for an inode (mutable + inherent).
        ///
        /// Used when a file is being fully re-indexed or removed — clears everything
        /// including file-inherent signals like CorruptFile and LosslessRemux.
        pub fn clear_all_corpus_signals(conn: &rusqlite::Connection, inode: i64) {
            use crate::meta::signals::store::CorpusSignalStore;
            $(let _ = <$m_type>::clear_by_inode(conn, inode);)*
            $(let _ = <$i_type>::clear_by_inode(conn, inode);)*
        }

    };

    // --- Internal helpers ---

    (@table_kind $slug:literal) => {
        // NOTE: macro metavariable literals don't re-match specific literal arms,
        // so we use a const function instead of pattern arms.
        table_kind_for_slug($slug)
    };
}

/// Classify a signal table by slug. Decision signals represent operator
/// choices (canonical_tag, expected_overlap, etc.) and are reconciled like
/// Core tables (ADD COLUMN only, never DROP+CREATE).
pub const fn table_kind_for_slug(slug: &str) -> TableKind {
    const fn bytes_eq(a: &[u8], b: &[u8]) -> bool {
        if a.len() != b.len() { return false; }
        let mut i = 0;
        while i < a.len() {
            if a[i] != b[i] { return false; }
            i += 1;
        }
        true
    }
    if bytes_eq(slug.as_bytes(), b"canonical_tag")
        || bytes_eq(slug.as_bytes(), b"expected_overlap")
        || bytes_eq(slug.as_bytes(), b"expected_duplicate")
        || bytes_eq(slug.as_bytes(), b"expected_missing_tag")
    {
        TableKind::Decision
    } else {
        TableKind::Computed
    }
}

// ============================================================================
// Signal Registry Declaration
// ============================================================================

use super::data::*;

signal_registry! {
    corpus {
        mutable {
            FileInCorpus(FileInCorpusSignal, "file_in_corpus"),
            UnindexedFile(UnindexedFileSignal, "unindexed_file"),
            HealthyFile(HealthyFileSignal, "healthy_file"),
            MtimeOnlyMismatch(MtimeOnlyMismatchSignal, "mtime_only_mismatch"),
            MissingDirectory(MissingDirectorySignal, "missing_directory"),
            MissingFile(MissingFileSignal, "missing_file"),
            MovedFile(MovedFileSignal, "moved_file"),
            DeployReady(DeployReadySignal, "deploy_ready"),
            DeployedHealthy(DeployedHealthySignal, "deployed_healthy"),
            SidecarDeployReady(SidecarDeployReadySignal, "sidecar_deploy_ready"),
            OutOfBandTagSync(OutOfBandTagSyncSignal, "oob_tag_sync"),
            OutOfBandTagConflict(OutOfBandTagConflictSignal, "oob_tag_conflict"),
            SubparDuplicate(SubparDuplicateSignal, "subpar_duplicate"),
            ArtistNeedsPlural(ArtistNeedsPluralSignal, "artist_needs_plural"),
            CompoundTag(CompoundTagSignal, "compound_tag"),
            PathTagMismatch(PathTagMismatchSignal, "path_tag_mismatch"),
            ExternalMatch(ExternalMatchSignal, "external_match"),
            ReleasePacking(ReleasePackingSignal, "release_packing"),
            UnmatchedCorpusTrack(UnmatchedCorpusTrackSignal, "unmatched_corpus_track"),
            ExpectedMissingTag(ExpectedMissingTagSignal, "expected_missing_tag"),
            MusicBrainzTagged(MusicBrainzTaggedSignal, "musicbrainz_tagged"),
        }
        inherent {
            CorruptFile(CorruptFileSignal, "corrupt_file"),
            LosslessRemux(LosslessRemuxSignal, "lossless_remux"),
        }
    }
    aggregate {
        CanonicalTag(CanonicalTagSignal, "canonical_tag"),
        ExpectedOverlap(ExpectedOverlapSignal, "expected_overlap"),
        ExpectedDuplicate(ExpectedDuplicateSignal, "expected_duplicate"),
        LibraryLeftover(LibraryLeftoverSignal, "library_leftover"),
        LibraryStale(LibraryStaleSignal, "library_stale"),
        FingerprintOverlap(FingerprintOverlapSignal, "fingerprint_overlap"),
        MetadataDuplicate(MetadataDuplicateSignal, "metadata_duplicate"),
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
        DiscExtraction(DiscExtractionSignal, "disc_extraction"),
        UnfilledReleaseSlot(UnfilledReleaseSlotSignal, "unfilled_release_slot"),
        PackedRelease(PackedReleaseSignal, "packed_release"),
        PackingKnot(PackingKnotSignal, "packing_knot"),
        AlternativeReleasePacking(AlternativeReleasePackingSignal, "alternative_release_packing"),
        VariousArtistsOverride(VariousArtistsOverrideSignal, "various_artists_override"),
        PinnedReleaseConflict(PinnedReleaseConflictSignal, "pinned_release_conflict"),
        PinnedReleasePackFailure(PinnedReleasePackFailureSignal, "pinned_release_pack_failure"),
        SameRecordingDifferentRelease(SameRecordingDifferentReleaseSignal, "same_recording_different_release"),
    }
}
