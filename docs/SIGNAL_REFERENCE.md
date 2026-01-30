# Signal Reference

> **Maintenance Requirement**: Adding, removing, or changing signal semantics MUST be
> reflected in this document. Each signal type must have documented emitters and clearers.

## Overview

Signals are atomic facts about corpus state. They follow these principles:
- **Individual**: One signal per file/track/issue (no aggregate counts)
- **Idempotent**: Creating an existing signal is a no-op
- **Witnessed**: All signal operations require a witness token

---

## Corpus File Signals (per-file)

| Signal | Emitted By | Cleared By | Meaning |
|--------|------------|------------|---------|
| FileInCorpus | ScanCorpusDirectory | ClearExistingObservationState | File discovered on disk |
| UnindexedFile | DeriveDirectorySignals | DeriveDirectorySignals, mutations | On disk but not in index |
| MissingFile | DeriveDirectorySignals | DeriveDirectorySignals, mutations | In index but not on disk |
| HealthyFile | DeriveDirectorySignals | DeriveDirectorySignals, mutations | In corpus, indexed, mtime matches, no OOB signals |
| CorruptFile | VerifyTags, IndexFileFromPath, Transcode | MoveToStash, DropFromIndex | Tag read or waveform decode failed |
| ShitFormat | IndexFileFromPath, Transcode | Transcode (to Opus/FLAC) | Non-Vorbis container (MP3, M4A, WAV, etc.) |
| OutOfBandTagSync | VerifyTags | VerifyTags, resolution mutations | One-way tag difference (syncable) |
| OutOfBandTagConflict | VerifyTags, TagEditAndFlush | VerifyTags, resolution mutations | Two-way tag conflict |
| MtimeOnlyMismatch | VerifyTags | VerifyTags, AcknowledgeMtimeOnly | Mtime changed, tags identical |
| InodeChanged | ScanCorpusDirectory | AcknowledgeInodeChanged | File was replaced (same path, new inode) |

---

## Library File Signals (per-file)

| Signal | Emitted By | Cleared By | Meaning |
|--------|------------|------------|---------|
| LibraryLeftover | DeriveDeployHealthSignals | UpdateDeploySignals, mutations | Library file with no corpus backing |
| LibraryStale | DeriveDeployHealthSignals | UpdateDeploySignals, LibraryMove | Library file at wrong path |
| DeployReady | DeriveCorpusDeployStatus | UpdateDeploySignals, HardLink | Healthy corpus file not deployed |
| DeployedHealthy | DeriveCorpusDeployStatus, UpdateDeploySignals | DeriveCorpusDeployStatus | Healthy corpus file correctly deployed |

---

## Aggregate Signals

Aggregate signals group multiple tracks by a shared characteristic. They use set reconciliation for efficient recomputation.

| Signal | Emitted By | Cleared By | Meaning |
|--------|------------|------------|---------|
| FingerprintDuplicate | DetectFingerprintDuplicates | DetectFingerprintDuplicates | Tracks with identical fingerprints |
| DuplicateInode | DetectDuplicateInodes | DetectDuplicateInodes | Tracks sharing same inode |
| MissingTag | DetectMissingTags | DetectMissingTags | Tracks missing required tags |
| MetadataDuplicate | DetectMetadataDuplicates | DetectMetadataDuplicates | Tracks with identical tag sets |
| TagCanonicity | DetectTagCanonicalizations | DetectTagCanonicalizations, TagEditDb | Similar tags needing unification |
| CompoundTagValue | DetectCompoundTagValues | DetectCompoundTagValues | Tags with separators needing split |
| InconsistentAlbumArtist | DetectInconsistentAlbumArtist | DetectInconsistentAlbumArtist, TagEditDb | Album with inconsistent artist |
| DeployConflict | DetectDeployConflicts | DetectDeployConflicts | Multiple tracks mapping to same library path |

---

## Signal Lifecycle

### Creation
```
ensure_file_signal_if_missing(db, sender, signal_type, key, witness)
  └─ Checks DB first (freshness optimization)
  └─ Queues INSERT OR IGNORE if signal doesn't exist
```

### Clearing
```
clear_file_signal_if_present(db, sender, signal_type, key, witness)
  └─ Checks DB first (freshness optimization)
  └─ Queues DELETE if signal exists
```

### Aggregate Reconciliation
```
reconcile_aggregate_signals(db, sender, signal_type, computed, witness)
  └─ Compares computed vs. stored
  └─ Clears stale (in DB, not computed)
  └─ Creates new (computed, not in DB)
  └─ Updates changed (both, but different track_ids)
  └─ Skips unchanged (both, same track_ids)
```

---

## Signal Design Principles

From `CLAUDE.md`:

1. **Signals must be small and individual** - One file/track per signal
2. **No aggregate signals for counts** - Compute counts via SQL at query time
3. **Signals are facts, not actions** - They describe state, not what to do
4. **Mutual exclusion where appropriate** - OOB signals (Sync/Conflict/MtimeOnly) are mutually exclusive

### Good Signals
- `UnindexedFile` for path X (one file)
- `FingerprintDuplicate` for fingerprint Z (one group of tracks)

### Bad Signals (DO NOT CREATE)
- `LibraryHealthSummary` (aggregate counts)
- Any "summary" signal that counts other signals
