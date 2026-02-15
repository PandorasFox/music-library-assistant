# Signal Reference

> **Maintenance Requirement**: Adding, removing, or changing signal semantics MUST be
> reflected in this document. Each signal type must have documented emitters and clearers.

## Source Location

Signal types are defined in `src/meta/signals/types.rs`:
- `SignalType` - top-level enum (`CorpusFile(CorpusFileSignalType)`, `Aggregate(AggregateSignalType)`)
- `CorpusFileSignalType` - per-file signal variants
- `AggregateSignalType` - aggregate signal variants
- `Signal`, `AggregateSignal` - signal data structs
- `SignalSummary`, `CorpusSummary` - summary types for UI display

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
| MissingDirectory | ScheduleSecondLevelDerivations | ScheduleSecondLevelDerivations, DropDirectoryFromIndex | Indexed directory no longer on disk |
| HealthyFile | DeriveDirectorySignals | DeriveDirectorySignals, mutations | In corpus, indexed, mtime matches, no OOB signals |
| CorruptFile | VerifyTags, VerifyAudio, IndexFileFromPath, Transcode | VerifyAudio (if valid), MoveToStash, DropFromIndex | Tag read or audio decode failed |
| ShitFormat | IndexFileFromPath, DetectShitFormats | Transcode (to Opus/FLAC), DetectShitFormats | Non-Vorbis container (MP3, M4A, WAV, etc.) |
| SubparDuplicate | AnalyzeFingerprintOverlaps | AnalyzeFingerprintOverlaps, MoveToStash | Track is outranked by a better version in its duplicate group (SubparFormat, SubparBitrate, or SubparSampleRate). Never emitted for equivalent-tier ties |
| OutOfBandTagSync | VerifyTags | VerifyTags, resolution mutations | One-way tag difference (syncable) |
| OutOfBandTagConflict | VerifyTags | VerifyTags, resolution mutations | Two-way tag conflict |
| MtimeOnlyMismatch | VerifyTags | VerifyTags, AcknowledgeMtimeOnly | Mtime changed, tags identical |
| InodeChanged | ScanCorpusDirectory | AcknowledgeInodeChanged | File was replaced (same path, new inode) |
| ExpectedMissingTag | EmitExpectedMissingTag | — | Operator-confirmed expected missing tag (persistent suppression). Table: `signal_expected_missing_tag`. Suppresses MissingAlbumSingleSignal for this inode in DetectMissingTags |

---

## Inbox File Signals (per-file)

| Signal | Emitted By | Cleared By | Meaning |
|--------|------------|------------|---------|
| FileInInbox | ScanCorpusDirectory (zone=inbox) | ClearExistingObservationState | File discovered on disk in inbox/ |
| InboxUnindexed | DeriveInboxSignals | DeriveInboxSignals | On disk in inbox but not in index |
| InboxHealthy | DeriveInboxSignals | DeriveInboxSignals | In inbox, indexed, ready for operations |

---

## Database State Flags

These are not signals but database columns that track synchronization state.

| Flag | Set By | Cleared By | Meaning |
|------|--------|------------|---------|
| needs_disk_flush | ApplyTagOps | ApplyDbTagsToDisk | DB tags changed but not yet synced to disk file |

### Recovery via needs_disk_flush

Tracks with `needs_disk_flush = TRUE` can be recovered via the OOB modal:

```sql
SELECT * FROM tracks WHERE needs_disk_flush = 1;
```

For each track, re-queue an `ApplyDbTagsToDisk` mutation. Since `ApplyDbTagsToDisk` reads from the database (source of truth), it's idempotent and can be safely re-run.

---

## Library File Signals (per-file)

| Signal | Emitted By | Cleared By | Meaning |
|--------|------------|------------|---------|
| LibraryLeftover | DeriveDeployHealthSignals | UpdateDeploySignals, mutations | Library file with no corpus backing |
| LibraryStale | DeriveDeployHealthSignals | UpdateDeploySignals, LibraryMove | Library file at wrong path |
| DeployReady | DeriveCorpusDeployStatus | UpdateDeploySignals, HardLink | Healthy corpus file not deployed. Metadata: `{ "deploy_path": "..." }` |
| DeployedHealthy | DeriveCorpusDeployStatus, UpdateDeploySignals | DeriveCorpusDeployStatus | Healthy corpus file correctly deployed. Metadata: `{ "library_path": "{library_name}/..." }` |

---

## Aggregate Signals

Aggregate signals group multiple tracks by a shared characteristic. They use set reconciliation for efficient recomputation.

| Signal | Emitted By | Cleared By | Meaning |
|--------|------------|------------|---------|
| FingerprintOverlap | DetectFingerprintOverlaps | DetectFingerprintOverlaps | Tracks with identical fingerprints (internal signal) |
| CrossSourceOverlap | DetectCrossSourceOverlaps | DetectCrossSourceOverlaps | Fingerprint overlaps spanning different source directories (from config `dir` stanzas). Key: sorted source pair, e.g., "web/releases/bandcamp\|web/releases/indie". Within-source overlaps are ignored. Metadata: `source_a`, `source_b`, `*_priority`, `*_can_stash`, `overlap_count`, `fingerprint_keys[]`, `track_pairs[]` |
| DuplicateInode | DetectDuplicateInodes | DetectDuplicateInodes | Tracks sharing same inode |
| MissingTag | DetectMissingTags | DetectMissingTags | Tracks missing required tags. Files with ALBUM missing but ARTIST+TITLE present (and not suppressed by ExpectedMissingTag) are routed to MissingAlbumSingleSignal instead |
| MissingAlbumSingleSignal | DetectMissingTags | DetectMissingTags | Tracks missing ALBUM tag but having ARTIST+TITLE. Key: lowercased artist name. Data (BLOB): artist name + tracks list. Suppressed for inodes with ExpectedMissingTag |
| MetadataDuplicate | DetectMetadataDuplicates | DetectMetadataDuplicates | Tracks with identical tag sets |
| TagCanonicity | DetectTagCanonicalizations | DetectTagCanonicalizations | Similar tags needing unification |
| CompoundTag | DetectCompoundTagsForInode | DetectCompoundTagValues (clears all before spawning) | Per-file signal for tags matching a `SplitRule` in the priority chain. `separator` field contains either the literal separator string (e.g., `";"`) or a collaboration keyword label (e.g., `"feat."`, `"vs."`). Metadata: `{ inode, compounds: [{ tag_name, compound_value, split_parts, separator }] }` |
| CanonicalTag | EmitCanonicalTag | - | Operator-confirmed canonical tag value (whitelist). Key: `{tag_name}:{tag_value}`. Prevents compound detection from flagging this value. Also suppresses TagCanonicity collision groups where any variant has a CanonicalTag |
| ExpectedOverlap | EmitExpectedOverlap | - | Operator-confirmed expected source pair overlap (whitelist). Key: sorted `"source_a\|source_b"` pair. Suppresses CrossSourceOverlap signal emission for this pair in DetectCrossSourceOverlaps. Also clears any existing CrossSourceOverlap signal for the pair when emitted |
| ExpectedDuplicate | EmitExpectedDuplicate | - | Operator-confirmed expected fingerprint overlap (whitelist). Key: fingerprint text (same key space as RedundantDuplicate). Suppresses RedundantDuplicate and SubparDuplicate signal emission for this fingerprint group in AnalyzeFingerprintOverlaps. Also clears any existing RedundantDuplicate signal for the key when emitted |
| InconsistentAlbumArtist | DetectInconsistentAlbumArtist | DetectInconsistentAlbumArtist | Album with inconsistent artist. Suppressed when any track in the group has `FLAGCOMPILATION=0` tag |
| RedundantDuplicate | AnalyzeFingerprintOverlaps | AnalyzeFingerprintOverlaps | Group of files with identical fingerprints and equivalent quality tier (same format class + metric). Requires operator choice — neither file is subpar. Key: fingerprint text. Data (bincode): `file_type`, `inodes[]`, `paths[]` |
| EmbeddableAlbumArt | DetectEmbeddableAlbumArt | DetectEmbeddableAlbumArt, EmbedAlbumArt | Directory with sidecar image and audio files lacking embedded art. Key: relative directory path. Metadata (bincode): `image_path`, `image_filename`, `artless_inodes[]`, `artless_paths[]` |
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

### GC Backstop

`DeriveCorpusSignals` includes a GC pass that clears orphaned corpus signals. After computing the known inode universe (disk inodes ∪ indexed inodes), it scans each corpus signal table for inodes outside that universe and deletes them. This catches signals that persist due to mutations that previously failed to return their affected inodes, or any future bugs in the post-mutation signal clearing pipeline.

Signal tables scanned: UnindexedFile, MissingFile, MovedFile, HealthyFile, CorruptFile, ShitFormat, MtimeOnlyMismatch, OutOfBandTagSync, OutOfBandTagConflict, SubparDuplicate, CompoundTag, DeployReady, DeployedHealthy, MissingDirectory, ExpectedMissingTag. FileInCorpus is excluded (it IS the disk observation).

### Good Signals
- `UnindexedFile` for path X (one file)
- `FingerprintOverlap` for fingerprint Z (one group of tracks)
- `CrossSourceOverlap` for source pair (groups tracks by configured source directory)

### Bad Signals (DO NOT CREATE)
- `LibraryHealthSummary` (aggregate counts)
- Any "summary" signal that counts other signals
