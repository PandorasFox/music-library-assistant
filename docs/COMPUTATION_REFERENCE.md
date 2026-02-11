# Computation Reference

> **Maintenance Requirement**: Any changes to computation logic, signal emission, or spawn
> behavior MUST be reflected in this document. Update the tables before or alongside code changes.

## Source Location

All computation code lives in `src/meta/computations/`:
- **Enum & dispatch**: `mod.rs` (Computation enum, `execute_single()`)
- **Witness**: `types.rs` (ComputationWitness sealed module)
- **Helpers**: `helpers.rs` (signal emission/clearing helpers)
- **Stats**: `stats.rs` (thread-local stats + read-only DB connections)
- **Asleep phase**: `asleep/mod.rs`, `asleep/executors.rs`
- **Awakening phase**: `awakening/mod.rs`, `awakening/executors.rs`
- **Awake phase**: `awake/mod.rs`, `awake/schedule.rs`, `awake/duplicates.rs`, `awake/tags.rs`, `awake/deploy.rs`, `awake/formats.rs`

## Phase Overview

MM uses three-phase computations with compile-time enforced boundaries:

| Phase | Purpose | Triggers |
|-------|---------|----------|
| **Asleep** | Pure corpus filesystem observation without inference | Startup, periodic eyeball |
| **Awakening** | First-level derivations comparing observations to index | After Asleep completes |
| **Awake** | Full-corpus analysis requiring complete awareness | After Awakening completes |

**Phase Boundary Enforcement**: Each phase has its own `Result` struct with a `spawn: Vec<PhaseComputation>` field that only accepts that phase's computations. Attempting to spawn a computation from a different phase will result in a compile error.

---

## Computations by Phase

### Asleep Phase

| Computation | Description |
|-------------|-------------|
| ClearExistingObservationState | Clear stale FileInCorpus signals before fresh scan |
| WalkCorpus | Enumerate directories, spawn per-directory scans. Has `force_check` parameter. |
| ScanCorpusDirectory | Collect disk state, emit FileInCorpus signals. Has `force_check` parameter. |
| VerifyMtime | Check file modification times for changes |
| VerifyTags | Verify disk tags match indexed tags, emit classification signals |

### Awakening Phase

| Computation | Description |
|-------------|-------------|
| ScheduleSecondLevelDerivations | Orchestrator for per-directory derivation work |
| DeriveDirectorySignals | Derive UnindexedFile, MissingFile, HealthyFile signals |
| UpdateCorpusFileSignals | Lightweight per-file signal update (post-mutation) |
| UpdateLibraryFileSignals | Library-side signal updates |
| UpdateDeploySignals | Update deployment status signals |
| WalkLibrary | Enumerate library directories |
| ScanLibraryDirectory | Scan library, store results in DB |

### Awake Phase

| Computation | Description |
|-------------|-------------|
| ScheduleContentAnalysis | Orchestrator: spawns all detection computations |
| DetectFingerprintOverlaps | Find tracks with identical fingerprints |
| DetectDuplicateInodes | Find tracks sharing same inode |
| DetectMissingTags | Find tracks missing required tags |
| DetectMetadataDuplicates | Find tracks with identical tag sets |
| DetectTagCanonicalizations | Find tag canonicalization opportunities |
| DetectInconsistentAlbumArtist | Find inconsistent album_artist across albums |
| DetectCompoundTagValues | Orchestrator: spawns DetectCompoundTagsForInode for each corpus inode. Parallelizes expensive regex work across worker threads. |
| DetectCompoundTagsForInode | Per-inode: detects compound tags (separator-based AND featuring patterns). Emits per-file CompoundTag signals. Skips CanonicalTag whitelisted values. |
| DetectShitFormats | Find files with non-Vorbis containers (MP3, M4A, etc) |
| AnalyzeFingerprintOverlaps | Analyze fingerprint overlaps for similarity, variants, quality |
| DetectCrossSourceOverlaps | Cluster FingerprintOverlap signals by source directory (from config `dir` stanzas). Within-source overlaps ignored. |
| DetectDeployConflicts | Detect path collisions in deployment |
| DeriveDeployHealthSignals | Derive library health signals (per library) |
| DeriveCorpusDeployStatus | Derive corpus deploy status |

---

## Computation Signal Matrix

### Asleep Phase Computations

| Computation | Spawns | Signals Emitted | Signals Cleared |
|-------------|--------|-----------------|-----------------|
| ClearExistingObservationState | — | — | FileInCorpus (all) |
| WalkCorpus | ScanCorpusDirectory × N (propagates `force_check`) | — | — |
| ScanCorpusDirectory | VerifyMtime (if mtime changed, normal mode) or VerifyTags + VerifyAudio (all indexed, if `force_check=true`) | FileInCorpus | — | Also indexes directory entry (is_dir=1) in files table |
| VerifyMtime | VerifyTags (if mtime differs) | — | — |
| VerifyTags | — | OutOfBandTagConflict, OutOfBandTagSync, MtimeOnlyMismatch, CorruptFile | OutOfBandTagConflict, OutOfBandTagSync, MtimeOnlyMismatch (mutual exclusion) |
| VerifyAudio | — | CorruptFile | CorruptFile (if audio valid) |

### Awakening Phase Computations

| Computation | Spawns | Signals Emitted | Signals Cleared |
|-------------|--------|-----------------|-----------------|
| ScheduleSecondLevelDerivations | DeriveDirectorySignals × N, WalkLibrary × N | MissingDirectory | MissingDirectory (if dir exists again) |
| DeriveDirectorySignals | — | UnindexedFile, MissingFile, HealthyFile | UnindexedFile, MissingFile, HealthyFile (stale); skips HealthyFile for OOB-flagged files |
| UpdateCorpusFileSignals | — | FileInCorpus, UnindexedFile, MissingFile, HealthyFile | FileInCorpus, UnindexedFile, MissingFile, HealthyFile |
| UpdateLibraryFileSignals | — | — | LibraryLeftover, LibraryStale |
| UpdateDeploySignals | — | DeployedHealthy | DeployReady, LibraryLeftover, LibraryStale |
| WalkLibrary | ScanLibraryDirectory × N | — | files (source='library') |
| ScanLibraryDirectory | — | — | — |

### Awake Phase Computations

| Computation | Spawns | Signals Emitted | Signals Cleared |
|-------------|--------|-----------------|-----------------|
| ScheduleContentAnalysis | All detection computations (except fingerprint-dependent) | — | — |
| DetectFingerprintOverlaps | AnalyzeFingerprintOverlaps, DetectCrossSourceOverlaps (after wait_for_queue_drain) | FingerprintOverlap | FingerprintOverlap (stale) |
| DetectDuplicateInodes | — | DuplicateInode | DuplicateInode (stale) |
| DetectMissingTags | — | MissingTag | MissingTag (all, then recreate) |
| DetectMetadataDuplicates | — | MetadataDuplicate | MetadataDuplicate (all, then recreate) |
| DetectTagCanonicalizations | — | TagCanonicity | TagCanonicity (all, then recreate) |
| DetectCompoundTagValues | DetectCompoundTagsForInode (per inode) | — | CompoundTag (all, before spawning) |
| DetectCompoundTagsForInode | — | CompoundTag (per-file) | — |
| DetectInconsistentAlbumArtist | — | InconsistentAlbumArtist | InconsistentAlbumArtist (all, then recreate) |
| DetectShitFormats | — | ShitFormat | ShitFormat (all, then recreate) |
| AnalyzeFingerprintOverlaps | — | SubparDuplicate | SubparDuplicate (all, then recreate) |
| DetectCrossSourceOverlaps | — | CrossSourceOverlap (keyed by sorted source pair, e.g., "bandcamp\|indie") | CrossSourceOverlap (all, then recreate) |
| DetectDeployConflicts | — | DeployConflict | DeployConflict (all, then recreate). Uses inode-based signal lookup (signal.inode + metadata path). |
| DeriveDeployHealthSignals | — | LibraryLeftover, LibraryStale | LibraryLeftover, LibraryStale. Masks stale-conflicts: if a stale file's expected path is already occupied by a different inode, no stale signal is emitted (the LibraryMove would always fail). |
| DeriveCorpusDeployStatus | — | DeployReady, DeployedHealthy | DeployReady, DeployedHealthy. Computes stale status inline from library files table (no dependency on LibraryStale signals). Clears both signal types before writing to avoid INSERT OR IGNORE staleness. DeployedHealthy metadata includes `library_path`. Skips conflict losers: files whose computed deploy path is claimed by 2+ corpus files are excluded from DeployReady (they would fail to deploy anyway). |

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

### Aggregate Signal Reconciliation

Bulk detection computations use set reconciliation:
1. Compute current signal set
2. Compare to existing signals in DB
3. Clear stale, create new, update changed, skip unchanged

### Witness-Based Emission

All signal operations require a witness token (`ComputationWitness` or `MutationExecutionWitness`) ensuring signals are only emitted from authorized contexts.
