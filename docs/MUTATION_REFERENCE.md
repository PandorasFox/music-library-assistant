# Mutation Reference

> **Maintenance Requirement**: Any changes to mutation behavior, spawned computations, or
> signal operations MUST be reflected in this document. Update the tables before or alongside code changes.

## Overview

Mutations are operator-confirmed changes to the corpus or index. All mutations:
- Require a `DecisionWitness` (created via `Witch::with_operator_decision()`)
- Execute in worker threads with write-capable database connections
- Can spawn follow-up computations (Awakening phase only)
- Can emit or clear signals

---

## Mutation Signal Matrix

### Tag Operations

| Mutation | Spawns Computations | Signals Emitted | Signals Cleared | Notes |
|----------|---------------------|-----------------|-----------------|-------|
| TagEditDb | — | — | TagCanonicity, InconsistentAlbumArtist | DB-only, no file touch |
| TagFlushToDisk | UpdateCorpusFileSignals | — | (per-file signals wiped) | Writes DB tags to disk |
| TagEditAndFlush | UpdateCorpusFileSignals | OutOfBandTagConflict (on failure), WaveformReadError | TagCanonicity, InconsistentAlbumArtist, (per-file wiped) | Combined edit + flush |

### Indexing Operations

| Mutation | Spawns Computations | Signals Emitted | Signals Cleared | Notes |
|----------|---------------------|-----------------|-----------------|-------|
| IndexTrack | UpdateCorpusFileSignals | WaveformReadError (if no fingerprint) | (per-file signals wiped) | Add track to index |
| IndexFileFromPath | UpdateCorpusFileSignals | WaveformReadError (if no fingerprint) | (per-file signals wiped) | Index by path |
| DropFromIndex | UpdateCorpusFileSignals | — | (per-file signals wiped) | Remove from index |
| UpdateTrack | UpdateCorpusFileSignals | — | (per-file signals wiped) | Update track metadata |

### Scan State Operations

| Mutation | Spawns Computations | Signals Emitted | Signals Cleared | Notes |
|----------|---------------------|-----------------|-----------------|-------|
| UpdateScanState | UpdateCorpusFileSignals | — | (per-file signals wiped) | Update scan_state entry |
| UpdateScanStatePath | — | — | — | Update path in scan_state |
| CleanupStaleScanState | — | — | — | Remove orphaned scan_state |

### File Operations

| Mutation | Spawns Computations | Signals Emitted | Signals Cleared | Notes |
|----------|---------------------|-----------------|-----------------|-------|
| Move | UpdateCorpusFileSignals × 2 | — | (signals for both paths wiped) | Move file within corpus |
| Copy | UpdateCorpusFileSignals × 2 | — | (signals for both paths wiped) | Copy file within corpus |
| MoveToStash | UpdateCorpusFileSignals | — | (per-file signals wiped) | Move to stash directory |
| UpdateTrackPath | UpdateCorpusFileSignals × 2 | — | (signals for both paths wiped) | Update path in index |
| Transcode | UpdateCorpusFileSignals × 2 | WaveformReadError | (signals for both paths wiped) | Transcode to new format |

### Deployment Operations

| Mutation | Spawns Computations | Signals Emitted | Signals Cleared | Notes |
|----------|---------------------|-----------------|-----------------|-------|
| HardLink | UpdateDeploySignals | — | DeployReady | Deploy to library |
| LibraryMove | — | — | LibraryStale | Library-internal move |

### OOB Resolution Operations

| Mutation | Spawns Computations | Signals Emitted | Signals Cleared | Notes |
|----------|---------------------|-----------------|-----------------|-------|
| AcknowledgeMtimeOnly | UpdateCorpusFileSignals | — | MtimeOnlyMismatch | Update scan_state mtime, acknowledge touch |
| AcknowledgeInodeChanged | UpdateCorpusFileSignals | — | InodeChanged | Update tracks.inode and scan_state for replaced files |
| ApplyDbTagsToDisk | UpdateCorpusFileSignals | — | OutOfBandTagSync, OutOfBandTagConflict, tag_mismatches | Write DB tags to file |
| AssimilateDiskTagsToDb | UpdateCorpusFileSignals | — | OutOfBandTagSync, OutOfBandTagConflict, tag_mismatches | Import disk tags to DB |

### Administrative Operations

| Mutation | Spawns Computations | Signals Emitted | Signals Cleared | Notes |
|----------|---------------------|-----------------|-----------------|-------|
| DbMigration | — | — | — | Schema migrations only |

---

## Key Patterns

### Post-Mutation Signal Flow

All successful mutations that affect file paths:
1. Spawn awakening-phase computations (`UpdateCorpusFileSignals` or `UpdateLibraryFileSignals`)
2. Before recomputation, per-file signals are wiped via `delete_signals_for_path()`
3. Spawned computations re-derive the current state

### Path-Based Filtering

Mutations spawn follow-up computations only for paths matching corpus/library criteria:
```rust
if is_corpus_path(&rel) {
    Some(Computation::UpdateCorpusFileSignals { path })
} else if is_library_path(&rel) {
    Some(Computation::UpdateLibraryFileSignals { path })
} else {
    None  // Skip non-corpus/library paths
}
```

### Signal Emission on Failure

Some mutations emit signals on failure rather than success:
- `TagEditAndFlush`: Emits `OutOfBandTagConflict` if stale edit detected
- Index mutations: Emit `WaveformReadError` if fingerprint extraction fails

### Canonicity Signal Clearing

`TagEditAndFlush` clears canonicity signals for affected tag types:
- Uses OLD values from edit to compute normalized keys
- Clears `TagCanonicity` for artist, album_artist, album, genre
- Clears `InconsistentAlbumArtist` when album_artist is edited
