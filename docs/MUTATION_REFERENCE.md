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

#### DB-First Pattern with Spawn Chaining

| Mutation | Spawns Mutations | Spawns Computations | Signals Emitted | Signals Cleared | Notes |
|----------|------------------|---------------------|-----------------|-----------------|-------|
| SetTrackTagsDb | **ApplyDbTagsToDisk** | — | — | — | Write tags to DB, set needs_disk_flush=true, spawn disk sync |
| ApplyDbTagsToDisk | — | UpdateCorpusFileSignals | — | (per-file signals wiped), needs_disk_flush | Read from DB, write to disk, clear needs_disk_flush |
| AssimilateDiskTagsToDb | — | UpdateCorpusFileSignals | — | (per-file signals wiped) | Read from disk, write to DB |

The DB-first pattern with spawn chaining:

1. **SetTrackTagsDb**: Write tags to DB (single track), set `needs_disk_flush=true`, **spawn** ApplyDbTagsToDisk.
2. **ApplyDbTagsToDisk**: Read tags from DB (source of truth), write to disk, clear `needs_disk_flush`.

All tag mutations operate on **single tracks** (not batches). Batch scheduling happens at the UI layer:

```rust
// Tag editor: 10 edited tracks = 10 SetTrackTagsDb queued
// Each spawns ApplyDbTagsToDisk → 10 more mutations auto-queued
for (track_id, tags) in edited_tracks {
    mutations.push(Mutation::SetTrackTagsDb { track_id, tags });
}

// OOB sync: 100 tracks = 100 individual mutations queued
for (track_id, path) in selected_tracks {
    mutations.push(Mutation::ApplyDbTagsToDisk { track_id, path });
}
```

Benefits:
- DB is always ahead of or in sync with disk
- If disk write fails/is interrupted, `needs_disk_flush=true` enables recovery via OOB flow
- Single-track mutations enable parallelism across worker threads
- Spawn chaining keeps DB write and disk sync atomic from user perspective
- UI shows granular progress (100 tracks = 100+ mutations visible)

Recovery flow: Query `SELECT * FROM tracks WHERE needs_disk_flush = 1`, queue ApplyDbTagsToDisk for each.

### Indexing Operations

| Mutation | Spawns Computations | Signals Emitted | Signals Cleared | Notes |
|----------|---------------------|-----------------|-----------------|-------|
| IndexTrack | UpdateCorpusFileSignals | CorruptFile (if no fingerprint), ShitFormat | (per-file signals wiped) | Add track to index |
| IndexFileFromPath | UpdateCorpusFileSignals | CorruptFile (on success if no fingerprint, **on failure**), ShitFormat | (per-file signals wiped) | Index by path |
| DropFromIndex | UpdateCorpusFileSignals | — | (per-file signals wiped) | Remove from index |
| DropDirectoryFromIndex | — | — | MissingFile × N, MissingDirectory | Drop directory and all contained files from index |
| UpdateTrack | UpdateCorpusFileSignals | — | (per-file signals wiped) | Update track metadata |

### File Entry Operations

| Mutation | Spawns Computations | Signals Emitted | Signals Cleared | Notes |
|----------|---------------------|-----------------|-----------------|-------|
| UpdateFileEntry | UpdateCorpusFileSignals | — | (per-file signals wiped) | Update file entry in files table |
| UpdateFilePath | — | — | — | Update path in files table |
| CleanupStaleFiles | — | — | — | Remove orphaned file entries |

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
| AcknowledgeMtimeOnly | UpdateCorpusFileSignals | — | MtimeOnlyMismatch | Update file mtime, acknowledge touch |
| AcknowledgeInodeChanged | UpdateCorpusFileSignals | — | InodeChanged | Update tracks.inode and files table for replaced files |

Note: ApplyDbTagsToDisk and AssimilateDiskTagsToDb are now single-track mutations documented in Tag Operations above. They clear OutOfBandTagSync, OutOfBandTagConflict, and tag_mismatch signals.

### Administrative Operations

| Mutation | Spawns Computations | Signals Emitted | Signals Cleared | Notes |
|----------|---------------------|-----------------|-----------------|-------|
| DbMigration | — | — | — | Schema migrations only |

---

## Key Patterns

### Atomic Tag Replacement

Tag operations use **diff-based atomic replacement** instead of DELETE-ALL-then-INSERT:

1. **Query existing tags** for the track
2. **Compute diff** between existing and desired tag sets
3. **DELETE only removed tags** (not in desired set)
4. **INSERT only new tags** (not in existing set)
5. **All within a single transaction** (rollback on failure)

Benefits:
- **No data loss on partial failure**: Transaction rolls back, leaving original state
- **No UNIQUE constraint violations**: Desired set is deduplicated before diffing
- **Minimal disk churn**: Unchanged tags aren't rewritten
- **Consistent state guaranteed**: Either all changes apply, or none do

This pattern is used in:
- `execute_set_track_tags()` - Direct tag replacement
- `execute_index_audio_file()` - Initial indexing with tags
- `execute_update_track_metadata()` - Metadata refresh with tags

Mutation generators (`TagCanonicalityState::mutations_with_paths()`, `CompoundSplitState::mutations_with_paths()`) also deduplicate tags via `TagSet` before creating mutations, preventing duplicate `(tag_name, tag_value)` pairs from reaching the DB layer.

### Spawn Chaining

Mutations can spawn follow-up mutations that execute automatically:

```
SetTrackTagsDb { track_id, tags }
    │
    ├── 1. Write tags to DB
    ├── 2. Set needs_disk_flush = true
    └── 3. Return SpawnedMutation via witness.spawn_mutation(ApplyDbTagsToDisk { ... })
              │
              └── Witch queues spawned mutation automatically
                        │
                        ├── 1. Read tags from DB
                        ├── 2. Write to disk
                        └── 3. Clear needs_disk_flush
```

The spawn chain is maintained via `SpawnedMutation`:
- Created via `MutationExecutionWitness::spawn_mutation(mutation)`
- The existence of `SpawnedMutation` IS the proof of authorization (factory pattern)
- Can only be created inside mutation execution context
- Executors return `Vec<SpawnedMutation>` in `MutationResult.spawn_mutations`
- Witch extracts inner mutation via `spawned.into_inner()` when queueing

Spawn chaining ensures:
- DB writes and disk syncs stay atomic from user perspective
- Interruption between steps leaves `needs_disk_flush=true` for recovery
- Single-track mutations enable parallel execution
- Type-level guarantee: spawned mutations must pass through authorized witness

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
- Index mutations: Emit `WaveformReadError` if fingerprint extraction fails
