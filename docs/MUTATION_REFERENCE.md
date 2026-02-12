# Mutation Reference

> **Maintenance Requirement**: Any changes to mutation behavior, spawned computations, or
> signal operations MUST be reflected in this document. Update the tables before or alongside code changes.

## Source Location

All mutation code lives in `src/meta/mutations/`:
- **Enum & dispatch**: `mod.rs` (Mutation enum, MutationToken sealed module)
- **Types**: `types.rs` (TagOp, PendingSignal, SignalClearScope, MutationResult, etc.)
- **Tag editing**: `tag_edit.rs` (ApplyTagOps, ApplyDbTagsToDisk, AssimilateDiskTagsToDb)
- **Indexing**: `indexing.rs` (IndexTrack, IndexFileFromPath, DropFromIndex, DropDirectoryFromIndex)
- **File operations**: `file_ops.rs` (Move, Copy, MoveToStash, HardLink, LibraryMove, etc.)
- **Transcoding**: `transcode.rs` (Transcode executor)
- **Migrations**: `migration.rs` (MigrationRegistry)

## Overview

Mutations are operator-confirmed changes to the corpus or index. All mutations:
- Require a `DecisionWitness` (created via `Witch::with_operator_decision()`)
- Execute in worker threads with write-capable database connections
- Can spawn follow-up computations (Awakening phase only)
- Can emit or clear signals

---

## Mutation Signal Matrix

### Tag Operations

#### Incremental Tag Operations with Validation

| Mutation | Spawns Mutations | Spawns Computations | Signals Emitted | Signals Cleared | Notes |
|----------|------------------|---------------------|-----------------|-----------------|-------|
| ApplyTagOps | **ApplyDbTagsToDisk** (per inode) | — | — | — | Incremental ops with validation, spawn disk sync per inode |
| ApplyDbTagsToDisk | — | UpdateCorpusFileSignals | — | (per-file signals wiped), needs_disk_flush | Read from DB, write to disk, clear needs_disk_flush |
| AssimilateDiskTagsToDb | — | UpdateCorpusFileSignals | — | (per-file signals wiped) | Read from disk, write to DB |

The incremental tag operation pattern:

1. **ApplyTagOps**: Apply incremental `TagOp` operations (add/drop/replace). For each inode:
   - Validate expected old_values exist (prevents stale overwrites)
   - Apply ops directly via `apply_index_tag_ops` (INSERT/UPDATE/DELETE)
   - Set `needs_disk_flush=true`
   - **Spawn** ApplyDbTagsToDisk for disk sync
2. **ApplyDbTagsToDisk**: Read tags from DB (source of truth), write to disk, clear `needs_disk_flush`.

TagOps map directly to SQL operations:
- `add` (old=None, new=Some) → `INSERT OR IGNORE`
- `drop` (old=Some, new=None) → `DELETE WHERE tag_value = old`
- `replace` (old=Some, new=Some) → `UPDATE SET tag_value = new WHERE tag_value = old`

Tag operations are batched at the UI layer, with coalescing at transaction commit:

```rust
// Tag editor: generates TagOps from changes
let ops = changes.iter().map(|c| TagOp::replace_tag(c.inode, &c.field, &c.old, &c.new));
mutations.push(Mutation::ApplyTagOps { ops });

// Multiple signals affecting same track: ops are coalesced at commit time
// (inode, tag_name, old_value) → last new_value wins
```

Benefits:
- **Validation**: Old values are checked before applying changes (prevents stale overwrites)
- **Coalescing**: Multiple signals affecting same track are properly merged
- DB is always ahead of or in sync with disk
- If disk write fails/is interrupted, `needs_disk_flush=true` enables recovery via OOB modal
- Spawn chaining keeps DB write and disk sync atomic from user perspective

Recovery process: Query `SELECT * FROM tracks WHERE needs_disk_flush = 1`, queue ApplyDbTagsToDisk for each.

### Indexing Operations

| Mutation | Spawns Computations | Signals Emitted | Signals Cleared | Notes |
|----------|---------------------|-----------------|-----------------|-------|
| IndexTrack | UpdateCorpusFileSignals | CorruptFile (if no fingerprint), ShitFormat | (per-file signals wiped) | Add track to index |
| IndexFileFromPath | UpdateCorpusFileSignals | CorruptFile (on success if no fingerprint, **on failure**), ShitFormat | (per-file signals wiped) | Index by path |
| DropFromIndex | UpdateCorpusFileSignals | — | All scope signals for inode (when inode known) | Remove from index |
| DropDirectoryFromIndex | — | — | MissingFile × N, MissingDirectory | Drop directory and all contained files from index |

### File Entry Operations

| Mutation | Spawns Computations | Signals Emitted | Signals Cleared | Notes |
|----------|---------------------|-----------------|-----------------|-------|
| UpdateFileEntry | UpdateCorpusFileSignals | — | (per-file signals wiped) | Update file entry in files table |
| UpdateFilePath | — | — | MutableOnly scope signals for inode | Update path in files table; handles both absolute and relative new_path |
| CleanupStaleFiles | — | — | — | Remove orphaned file entries |

### File Operations

| Mutation | Spawns Computations | Signals Emitted | Signals Cleared | Notes |
|----------|---------------------|-----------------|-----------------|-------|
| Move | UpdateCorpusFileSignals × 2 | — | (signals for both paths wiped) | Move file within corpus |
| Copy | UpdateCorpusFileSignals × 2 | — | (signals for both paths wiped) | Copy file within corpus |
| MoveToStash | UpdateCorpusFileSignals | — | All scope signals for discovered inode | Move to stash directory; discovers inode before move |
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
| AcknowledgeMtimeOnly | UpdateCorpusFileSignals | — | MtimeOnlyMismatch, MutableOnly scope signals for all track inodes | Update file mtime, acknowledge touch |
| AcknowledgeInodeChanged | UpdateCorpusFileSignals | — | InodeChanged | Update tracks.inode and files table for replaced files |

Note: ApplyDbTagsToDisk and AssimilateDiskTagsToDb are now single-track mutations documented in Tag Operations above. They clear OutOfBandTagSync, OutOfBandTagConflict, and tag_mismatch signals.

### Administrative Operations

| Mutation | Spawns Computations | Signals Emitted | Signals Cleared | Notes |
|----------|---------------------|-----------------|-----------------|-------|
| DbMigration | — | — | — | Schema migrations only |

### Signal Emission Operations

| Mutation | Spawns Computations | Signals Emitted | Signals Cleared | Notes |
|----------|---------------------|-----------------|-----------------|-------|
| EmitCanonicalTag | — | CanonicalTag | — | Whitelist a tag value as canonical (not compound) |

The EmitCanonicalTag mutation is used when an operator confirms that a compound-looking value (e.g., "Rinse & Repeat") is actually a single canonical entity (band name) and should not be split. It emits a CanonicalTag signal with key `{tag_name}:{tag_value}` (whitelist entry). Future compound detection runs check for CanonicalTag signals and skip whitelisted values.

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
- `execute_set_index_track_tags()` - Direct tag replacement
- `execute_index_audio_file()` - Initial indexing with tags

Mutation generators (`TagCanonicalityState::mutations_with_paths()`, `CompoundSplitState::mutations_with_paths()`) also deduplicate tags via `TagSet` before creating mutations, preventing duplicate `(tag_name, tag_value)` pairs from reaching the DB layer.

### Spawn Chaining

Mutations can spawn follow-up mutations that execute automatically:

```
ApplyTagOps { ops: [TagOp, ...] }
    │
    ├── 1. Group ops by inode
    ├── 2. For each inode:
    │       ├── Validate old_values exist
    │       ├── Apply changes
    │       ├── Write tags to DB
    │       ├── Set needs_disk_flush = true
    │       └── Return SpawnedMutation(ApplyDbTagsToDisk { inode, path })
    │
    └── Witch queues spawned mutations automatically (one per modified inode)
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

### Post-Mutation Signal Lifecycle

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
