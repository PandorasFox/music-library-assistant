# Computation Reference

> **Maintenance Requirement**: Any changes to computation logic, signal emission, or spawn
> behavior MUST be reflected in this document. Update the tables before or alongside code changes.

## Source Location

All computation code lives in `src/meta/computations/`:
- **Enum & dispatch**: `mod.rs` (Computation enum, `execute_single()`)
- **Witness**: `types.rs` (ComputationWitness sealed module)
- **Helpers**: `helpers.rs` (signal emission/clearing helpers)
- **Stats**: `stats.rs` (thread-local stats + read-only DB connections)
- **Observation phase**: `observation/mod.rs`, `observation/executors.rs`
- **Derivation phase**: `derivation/mod.rs`, `derivation/executors.rs`
- **Analysis phase**: `analysis/mod.rs`, `analysis/schedule.rs`, `analysis/duplicates.rs`, `analysis/tags.rs`, `analysis/deploy.rs`, `analysis/formats.rs`, `analysis/inbox_matches.rs`, `analysis/external_matches.rs`

## Phase Overview

MM uses three-phase computations with compile-time enforced boundaries:

| Phase | Purpose | Triggers |
|-------|---------|----------|
| **Observation** | Pure corpus filesystem observation without inference | Startup, periodic rescan |
| **Derivation** | First-level derivations comparing observations to index | After Observation completes |
| **Analysis** | Full-corpus analysis requiring complete awareness | After Derivation completes |

**Phase Boundary Enforcement**: Each phase has its own `Result` struct with a `spawn: Vec<PhaseComputation>` field that only accepts that phase's computations. Attempting to spawn a computation from a different phase will result in a compile error.

---

## Computations by Phase

### Observation Phase

| Computation | Description |
|-------------|-------------|
| WalkCorpus | Enumerate directories, spawn per-directory scans. Has `force_check` parameter. |
| ScanCorpusDirectory | Collect disk state, emit FileInCorpus signals, return observed inodes to Witch. Has `force_check` parameter. |
| VerifyMtime | Check file modification times for changes |
| VerifyTags | Verify disk tags match indexed tags, emit classification signals |

### Derivation Phase

| Computation | Description |
|-------------|-------------|
| ScheduleSecondLevelDerivations | Orchestrator for per-directory derivation work |
| DeriveDirectorySignals | Derive UnindexedFile, MissingFile, HealthyFile signals |
| UpdateCorpusFileSignals | Lightweight per-file signal update (post-mutation) |
| UpdateLibraryFileSignals | Library-side signal updates |
| UpdateDeploySignals | Update deployment status signals |
| WalkLibrary | Enumerate library directories, spawn per-directory scans |
| ScanLibraryDirectory | Scan library directory, return observed files to Witch |
| ReconcileLibraryFiles | Reconcile observed library files against DB (set reconciliation) |

### Analysis Phase

| Computation | Description |
|-------------|-------------|
| ScheduleContentAnalysis | Orchestrator: spawns all detection computations |
| DetectFingerprintOverlaps | Find tracks with identical fingerprints |
| DetectDuplicateInodes | Find tracks sharing same inode |
| DetectMissingTags | Find tracks missing required tags. Routes ALBUM-only-missing (with ARTIST+TITLE) to MissingAlbumSingleSignal; checks ExpectedMissingTag for suppression |
| DetectMetadataDuplicates | Find tracks with identical tag sets |
| DetectTagCanonicalizations | Find tag canonicalization opportunities |
| DetectInconsistentAlbumArtist | Find inconsistent album_artist across albums. Skips groups where any track has `COMPILATION=0` |
| DetectCompoundTagValues | Orchestrator: spawns DetectCompoundTagsForInode for each dirty corpus inode. Parallelizes detection across worker threads. |
| DetectCompoundTagsForInode | Per-inode: walks the priority-ordered `SplitRule` chain from `TagSplittingOpinions` (separator and collaboration keyword rules). First matching rule wins per tag value. Emits per-file CompoundTag signals. Skips CanonicalTag whitelisted values. |
| DetectShitFormats | Find files with non-Vorbis containers (MP3, M4A, etc). Uses dirty inode tracking — only checks recently (re)indexed inodes |
| DetectEmbeddableAlbumArt | Find directories with sidecar album art images alongside audio files lacking embedded pictures |
| DetectInboxCorpusMatches | Find inbox files matching corpus by fingerprint+duration similarity. Pre-computes quality classification (better/equivalent/subpar) using QualityTier with bitrate fuzz |
| DetectInboxTagCanonicity | Compare inbox tag values against corpus vocabulary. Flags inbox values whose normalized form matches a corpus value but whose exact spelling differs. Skips novel values (no corpus equivalent) and CanonicalTag whitelisted values. Full recompute each cycle |
| DetectInboxMissingTags | Detect inbox files missing required tags. Simplified version of DetectMissingTags: no ExpectedMissingTag suppression, no MissingAlbumSingle routing, ALBUM_ARTIST removed from required set when compilation-only. Reuses MissingTagData. Full recompute each cycle |
| DetectInboxCompoundTags | Single-pass compound tag detection for inbox files. Checks collaboration keywords + per-tag separators, enriches matching_parts against corpus vocabulary. No orchestrator/dirty-inode tracking (inbox is small). Full recompute each cycle |
| DetectPathTagMismatches | Detect files whose paths don't match their source dir's path-tag schema. Extracts tag values from path structure, compares against DB tags (case-insensitive). Emits per-file PathTagMismatch signals |
| DeriveExternalMatches | Derive external match signals from AcoustID results. Compares recording metadata (title, artist, album) against corpus tags using raw string equality. Emits per-file ExternalMatch signals with classification and diffs |
| DetectEmbeddedDiscNumbers | Detect ALBUM tags with embedded disc numbers (e.g., "Album, Disc 2"). Scans both corpus and inbox. Emits EmbeddedDiscNumber aggregate signals |
| AnalyzeFingerprintOverlaps | Analyze fingerprint overlaps for similarity, variants, quality tier partitioning |
| DetectCrossSourceOverlaps | Cluster FingerprintOverlap signals by source directory (from config `dir` stanzas). Within-source overlaps ignored. |
| DetectDeployConflicts | Detect path collisions in deployment |
| DetectReleaseOverlaps | Detect cross-source album-directory-level release overlaps |
| DeriveDeployHealthSignals | Derive library health signals (per library) |
| DeriveCorpusDeployStatus | Derive corpus deploy status |

---

## Computation Signal Matrix

### Observation Phase Computations

| Computation | Spawns | Signals Emitted | Signals Cleared |
|-------------|--------|-----------------|-----------------|
| WalkCorpus | ScanCorpusDirectory × N (propagates `force_check`) | — | — |
| ScanCorpusDirectory | VerifyMtime (if mtime changed, normal mode) or VerifyTags + VerifyAudio (all indexed, if `force_check=true`) | FileInCorpus (corpus zone), FileInInbox (inbox zone) | — | Also indexes directory entry (is_dir=1) in files table with read guard (skips write if entry already matches by zone+inode+mtime). Returns observed inodes to the Witch via Result (accumulated in tick(), consumed by queue_derivation_computations()). |
| VerifyMtime | VerifyTags (if mtime differs) | — | — |
| VerifyTags | — | OutOfBandTagConflict, OutOfBandTagSync, MtimeOnlyMismatch, CorruptFile | OutOfBandTagConflict, OutOfBandTagSync, MtimeOnlyMismatch (mutual exclusion) |
| VerifyAudio | — | CorruptFile | CorruptFile (if audio valid) |

### Derivation Phase Computations

| Computation | Spawns | Signals Emitted | Signals Cleared |
|-------------|--------|-----------------|-----------------|
| ScheduleSecondLevelDerivations | WalkLibrary × N | MissingDirectory | MissingDirectory (if dir exists again) |
| DeriveCorpusSignals | — | UnindexedFile, MissingFile, HealthyFile | UnindexedFile, MissingFile, HealthyFile (stale); skips HealthyFile for OOB-flagged files. **GC backstop**: clears orphaned signals for inodes not in disk ∪ index. Receives observed inodes from the Witch (accumulated from ScanCorpusDirectory results). |
| DeriveInboxSignals | — | InboxUnindexed, InboxHealthy | InboxUnindexed (stale). **Cascade-drop**: for indexed inbox inodes no longer on disk, drops all inbox state via DropInboxFileState (inbox_tags, files zone='inbox', FileInInbox, InboxUnindexed, InboxHealthy, InboxCorpusMatch, MovedFile). Disk presence is sole authority — GC backstop uses disk_set only (not disk ∪ indexed). Receives observed inodes from the Witch (accumulated from ScanCorpusDirectory results). |
| UpdateCorpusFileSignals | — | FileInCorpus, UnindexedFile, MissingFile, HealthyFile | FileInCorpus, UnindexedFile, MissingFile, HealthyFile |
| UpdateLibraryFileSignals | — | — | LibraryLeftover, LibraryStale |
| UpdateDeploySignals | — | DeployedHealthy | DeployReady, LibraryLeftover, LibraryStale |
| WalkLibrary | ScanLibraryDirectory × N | — | — |
| ScanLibraryDirectory | — | — | — | Returns observed library files to the Witch via Result (accumulated in tick()). No direct DB writes. |
| ReconcileLibraryFiles | — | — | — | Set reconciliation: compares observed files against DB. Upserts new/changed files, deletes stale files, skips unchanged. Queued by the Witch after derivation stage 1 drains (two-stage derivation transition). |

### Analysis Phase Computations

| Computation | Spawns | Signals Emitted | Signals Cleared |
|-------------|--------|-----------------|-----------------|
| ScheduleContentAnalysis | All detection computations (except fingerprint-dependent) | — | — |
| DetectFingerprintOverlaps | AnalyzeFingerprintOverlaps, DetectCrossSourceOverlaps (after wait_for_queue_drain) | FingerprintOverlap | FingerprintOverlap (stale) |
| DetectDuplicateInodes | — | DuplicateInode | DuplicateInode (stale) |
| DetectMissingTags | — | MissingTag, MissingAlbumSingleSignal | MissingTag (via hash-based reconciliation), MissingAlbumSingleSignal (via hash-based reconciliation). Files with ALBUM missing but ARTIST+TITLE present are routed to MissingAlbumSingleSignal (keyed by lowercased artist) instead of MissingTag. Checks ExpectedMissingTag to suppress known-acceptable missing-album inodes |
| DetectMetadataDuplicates | — | MetadataDuplicate | MetadataDuplicate (via hash-based reconciliation) |
| DetectTagCanonicalizations | — | TagCanonicity | TagCanonicity (via hash-based reconciliation). Loads `strip_album_format_suffixes` from config for album collision detection. Skips collision groups where any variant has a CanonicalTag signal |
| DetectCompoundTagValues | DetectCompoundTagsForInode (per inode) | — | CompoundTag (all, before spawning) |
| DetectCompoundTagsForInode | — | CompoundTag (per-file) | — |
| DetectInconsistentAlbumArtist | — | InconsistentAlbumArtist | InconsistentAlbumArtist (via hash-based reconciliation). Groups with `COMPILATION=0` on any track are suppressed (no signal emitted) |
| DetectShitFormats | — | ShitFormat | ShitFormat (via dirty inode tracking — only processes recently indexed inodes) |
| AnalyzeFingerprintOverlaps | — | SubparDuplicate, RedundantDuplicate | SubparDuplicate (via hash-based corpus reconciliation), RedundantDuplicate (via hash-based aggregate reconciliation). Uses enum-based equivalence-class partitioning (QualityTier = FormatClass + metric). Best tier with >1 file → RedundantDuplicate; lower tiers → SubparDuplicate with reason (SubparFormat, SubparBitrate, SubparSampleRate). Re-release elision: album checked before ISRC when catalog numbers absent. Skips fingerprint groups with an ExpectedDuplicate signal (operator whitelist). Skips groups entirely within a single source dir that has `interior-dupes false` in config |
| DetectEmbeddableAlbumArt | — | EmbeddableAlbumArt | EmbeddableAlbumArt (stale, via set reconciliation) |
| DetectInboxCorpusMatches | — | InboxCorpusMatch | InboxCorpusMatch (via hash-based corpus reconciliation). For each inbox file with fingerprint, finds corpus files within duration tolerance with similarity above threshold. Pre-computes quality classification (Better/Equivalent/Subpar) via QualityTier comparison with inbox_bitrate_fuzz_percent. Classification stored as SQL column + bincode BLOB field. `better` files pass through to inbox organize |
| DetectInboxTagCanonicity | — | InboxTagCanonicity | InboxTagCanonicity (via hash-based aggregate reconciliation). For each tag field (artist, album_artist, album, genre): normalizes inbox values, finds corpus matches, skips exact matches and CanonicalTag whitelisted values. Data stored as bincode BLOB |
| DetectInboxMissingTags | — | InboxMissingTag | InboxMissingTag (via hash-based aggregate reconciliation). Groups inbox files by album/directory, checks against configured required_tags. No suppression, no album-single routing. Data reuses MissingTagData bincode BLOB |
| DetectInboxCompoundTags | — | InboxCompoundTag | InboxCompoundTag (per-inode write/clear). Single-pass over inbox healthy inodes. Checks collaboration keywords + per-tag separators, enriches matching_parts against corpus vocabulary. Cleans up signals for inodes no longer healthy |
| DetectPathTagMismatches | — | PathTagMismatch | PathTagMismatch (via hash-based corpus reconciliation). For each corpus audio file with a matching source dir schema (direct or inherited), strips source dir prefix and extension, runs schema extraction, compares extracted tags against DB tags (case-insensitive). Emits StructureMismatch or ValueMismatch signals |
| DetectEmbeddedDiscNumbers | — | EmbeddedDiscNumber | EmbeddedDiscNumber (via hash-based aggregate reconciliation). Scans ALBUM tags from corpus_tags and inbox_tags for `,?\s*disc\s+(\d+)\s*$` pattern |
| DeriveExternalMatches | — | ExternalMatch | ExternalMatch (via hash-based corpus reconciliation). For each corpus inode with AcoustID matches, parses stored raw response JSON, compares recording title/artist/album against corpus tags (raw string equality), classifies as ExactMatch/ContentDiff/MetadataOnly. Triggered by EXTERNAL, TAGS, or FILES scope |
| DetectCrossSourceOverlaps | — | CrossSourceOverlap (keyed by sorted source pair, e.g., "bandcamp\|indie") | CrossSourceOverlap (via hash-based aggregate reconciliation). Skips source pairs with an ExpectedOverlap signal (operator whitelist) |
| DetectDeployConflicts | — | DeployConflict | DeployConflict (via hash-based aggregate reconciliation). Uses inode-based signal lookup (signal.inode + metadata path). |
| DetectReleaseOverlaps | — | ReleaseOverlap | ReleaseOverlap (via hash-based aggregate reconciliation). Groups healthy corpus files by album directory (parent of deploy path), partitions by (source_dir, release_dir). Only emits for cross-source overlaps (2+ configured sources targeting the same album directory). Intra-source overlaps are skipped. |
| DeriveDeployHealthSignals | — | LibraryLeftover, LibraryStale | LibraryLeftover, LibraryStale. Masks stale-conflicts: if a stale file's expected path is already occupied by a different inode, no stale signal is emitted (the LibraryMove would always fail). |
| DeriveCorpusDeployStatus | — | DeployReady, DeployedHealthy | DeployReady, DeployedHealthy (via corpus reconciliation — scalar inode existence check, no hash). DeployedHealthy metadata includes `library_path`. Files deployed at the wrong path (stale) are skipped — DeriveDeployHealthSignals handles those via LibraryStale. Skips conflict losers: files whose computed deploy path is claimed by 2+ corpus files are excluded from DeployReady. Also skips files whose album directory has a ReleaseOverlap signal (release overlap suppression). |

### Fingerprinting Limitations

Chromaprint requires a minimum audio duration to generate a usable fingerprint. The algorithm uses 4096-sample frames at 11025 Hz (~0.37 seconds each) with 2/3 overlap, and needs multiple frames to produce meaningful output. In practice, **audio under ~2 seconds cannot be fingerprinted**.

Files that are too short will have empty fingerprints stored in `audio_info`. These files:
- Are excluded from `DetectFingerprintOverlaps` (empty fingerprints are filtered out)
- Cannot participate in duplicate detection via fingerprint matching
- Will emit `CorruptFile` signals during indexing (fingerprint extraction failure)

This primarily affects sound effects, jingles, and very short intros. Such files must be deduplicated manually if needed.

---

## Key Architectural Patterns

### Signal Freshness Optimization

Computations use helper functions to avoid redundant writes:
- `ensure_file_signal_if_missing()` - Only queues write if signal doesn't exist
- `clear_file_signal_if_present()` - Only queues delete if signal exists

### Hash-Based Signal Reconciliation

Bulk detection computations use hash-based set reconciliation:
1. Compute current signal set with content hashes (SipHash of bincode bytes for BLOB types)
2. Fetch existing key→hash map from DB (via `data_hash` column)
3. Clear stale (in DB, not computed), create new, update changed (hash differs), skip unchanged (hash matches)
4. Returns `(cleared, new, updated, unchanged)` counts for logging

This eliminates redundant writes on steady-state cycles where signal data hasn't changed.

### Witness-Based Emission

All signal operations require a witness token (`ComputationWitness` or `MutationExecutionWitness`) ensuring signals are only emitted from authorized contexts.
