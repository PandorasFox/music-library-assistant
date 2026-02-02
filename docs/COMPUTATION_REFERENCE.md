# Computation Reference

> **Maintenance Requirement**: Any changes to computation logic, signal emission, or spawn
> behavior MUST be reflected in this document. Update the tables before or alongside code changes.

## Phase Overview

MLA uses three-phase computations with compile-time enforced boundaries:

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
| DetectCompoundTagValues | Find separators needing splits |
| DetectShitFormats | Find files with non-Vorbis containers (MP3, M4A, etc) |
| AnalyzeFingerprintOverlaps | Analyze fingerprint overlaps for similarity, variants, quality |
| ClusterDirectoryOverlaps | Cluster FingerprintOverlap signals by directory for UI resolution |
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
| ScanCorpusDirectory | VerifyMtime (if mtime changed, normal mode) or VerifyTags + VerifyAudio (all indexed, if `force_check=true`) | FileInCorpus | — |
| VerifyMtime | VerifyTags (if mtime differs) | — | — |
| VerifyTags | — | OutOfBandTagConflict, OutOfBandTagSync, MtimeOnlyMismatch, CorruptFile | OutOfBandTagConflict, OutOfBandTagSync, MtimeOnlyMismatch (mutual exclusion) |
| VerifyAudio | — | CorruptFile | CorruptFile (if audio valid) |

### Awakening Phase Computations

| Computation | Spawns | Signals Emitted | Signals Cleared |
|-------------|--------|-----------------|-----------------|
| ScheduleSecondLevelDerivations | DeriveDirectorySignals × N, WalkLibrary × N | — | — |
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
| DetectFingerprintOverlaps | AnalyzeFingerprintOverlaps, ClusterDirectoryOverlaps (after wait_for_queue_drain) | FingerprintOverlap | FingerprintOverlap (stale) |
| DetectDuplicateInodes | — | DuplicateInode | DuplicateInode (stale) |
| DetectMissingTags | — | MissingTag | MissingTag (all, then recreate) |
| DetectMetadataDuplicates | — | MetadataDuplicate | MetadataDuplicate (all, then recreate) |
| DetectTagCanonicalizations | — | TagCanonicity | TagCanonicity (all, then recreate) |
| DetectCompoundTagValues | — | CompoundTagValue | CompoundTagValue (all, then recreate) |
| DetectInconsistentAlbumArtist | — | InconsistentAlbumArtist | InconsistentAlbumArtist (all, then recreate) |
| DetectShitFormats | — | ShitFormat | ShitFormat (all, then recreate) |
| AnalyzeFingerprintOverlaps | — | SubparDuplicate | SubparDuplicate (all, then recreate) |
| ClusterDirectoryOverlaps | — | DirectoryOverlapCluster (cross-directory only, ≤ max_keys) | DirectoryOverlapCluster (all, then recreate) |
| DetectDeployConflicts | — | DeployConflict | DeployConflict (all, then recreate) |
| DeriveDeployHealthSignals | — | LibraryLeftover, LibraryStale | LibraryLeftover, LibraryStale |
| DeriveCorpusDeployStatus | — | DeployReady, DeployedHealthy | DeployReady, DeployedHealthy |

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
