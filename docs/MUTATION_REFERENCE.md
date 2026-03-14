# Mutation Reference

> **Maintenance Requirement**: Any changes to mutation behavior, spawned computations, or
> signal operations MUST be reflected in this document. Update the tables before or alongside code changes.

## Source Location

All mutation code lives in `src/meta/mutations/`:
- **Enum & dispatch**: `mod.rs` (Mutation enum, MutationToken sealed module)
- **Types**: `types.rs` (TagOp, PendingSignal, SignalClearScope, MutationResult, etc.)
- **Tag editing**: `tag_edit.rs` (ApplyTagOps, ApplyDbTagsToDisk, AssimilateDiskTagsToDb)
- **Indexing**: `indexing.rs` (IndexTrack, IndexFileFromPath, DropFromIndex, DropDirectoryFromIndex)
- **File operations**: `file_ops.rs` (Move, Copy, StashFromZone, StashLeftovers, HardLink, LibraryMove, etc.)
- **Transcoding**: `transcode.rs` (Transcode executor)
- **Config editing**: `config_edit.rs` (ApplyConfigEdits executor)
- **Schema reconciliation**: `db/reconciler.rs` (auto-detects and applies schema changes at startup)

## Overview

Mutations are operator-confirmed changes to the corpus or index. All mutations:
- Require a `DecisionWitness` (created via `Witch::with_operator_decision()`)
- Execute in worker threads with write-capable database connections
- Can spawn follow-up computations (Derivation phase only)
- Can emit or clear signals

---

## Execution Staging

When a transaction is confirmed, mutations are bucketed by execution stage and
executed phase-by-phase with drain barriers between each phase. This ensures
that DB writes complete before disk flushes, and disk flushes complete before
deployment operations.

| Stage | Order | Mutations |
|-------|-------|-----------|
| Config | 0 | ApplyConfigEdits, ApplyDirConfigEdit, ApplyBatchDirConfigEdits |
| DB | 1 | ApplyTagOps, AcknowledgeMtimeOnly, EmitCanonicalTag, EmitExpectedOverlap, EmitExpectedDuplicate, EmitExpectedMissingTag, IndexFileFromPath, UpdateFilePath, InboxToCorpus, InboxDirToCorpus, ApplyDbTagsToDisk |
| DiskFlush | 2 | Transcode, Move, StashFromZone, StashLeftovers, DropFromIndex, DropDirectoryFromIndex, ExportEditHistory |
| DiskDeploy | 3 | HardLink, LibraryMove |
| *(ChainEmitted)* | — | FlushTagsToDisk, AssimilateDiskTagsToDb, ClearEditHistory *(spawned during execution, never in transactions)* |

---

## Mutation Signal Matrix

### Tag Operations

#### Incremental Tag Operations with Validation

| Mutation | Spawns Mutations | Spawns Computations | Signals Emitted | Signals Cleared | Notes |
|----------|------------------|---------------------|-----------------|-----------------|-------|
| ApplyTagOps | **FlushTagsToDisk** (per inode) | — | — | — | Zone-aware: reads/writes corpus_tags or inbox_tags based on `zone` field. Incremental ops with validation, spawn disk sync per inode |
| ApplyDbTagsToDisk | — | UpdateCorpusFileSignals | — | (per-file signals wiped), needs_disk_flush | Zone-aware: reads from corpus_tags or inbox_tags based on `zone` field. Write to disk, clear needs_disk_flush |
| FlushTagsToDisk | — | UpdateCorpusFileSignals | — | (per-file signals wiped), needs_disk_flush | Zone-aware: validates committed tags from zone-appropriate table against expected_tags. Write to disk on match |
| AssimilateDiskTagsToDb | — | UpdateCorpusFileSignals | — | (per-file signals wiped) | Zone-aware: writes to corpus_tags or inbox_tags based on zone. Read from disk, write to DB |

The incremental tag operation pattern:

1. **ApplyTagOps**: Apply incremental `TagOp` operations (add/drop/replace). Carries `zone: Zone` to determine tag table. For each inode:
   - Validate expected old_values exist (prevents stale overwrites)
   - Apply ops directly via `apply_index_tag_ops` (INSERT/UPDATE/DELETE) to zone-appropriate tag table
   - Set `needs_disk_flush=true`
   - **Spawn** FlushTagsToDisk for disk sync (carries zone)
2. **FlushTagsToDisk**: Drain DB queue, validate committed tags against expected, write to disk, clear `needs_disk_flush`.
3. **ApplyDbTagsToDisk**: Read tags from zone-appropriate table (source of truth), write to disk, clear `needs_disk_flush`.

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
| UpdateFilePath | — | — | MutableOnly scope signals for inode | Update path in files table; handles both absolute and relative new_path. When `new_zone` is set and differs from `zone`, also updates zone column and migrates tags between tag tables (e.g. inbox_tags → corpus_tags) |
| CleanupStaleFiles | — | — | — | Remove orphaned file entries |

### File Operations

| Mutation | Spawns Computations | Signals Emitted | Signals Cleared | Notes |
|----------|---------------------|-----------------|-----------------|-------|
| Move | UpdateCorpusFileSignals × 2 | — | (signals for both paths wiped) | Move file within corpus |
| Copy | UpdateCorpusFileSignals × 2 | — | (signals for both paths wiped) | Copy file within corpus |
| StashFromZone | UpdateCorpusFileSignals | — | All scope signals for discovered inode | Operator-driven stash of corpus/inbox files; discovers inode before move |
| StashLeftovers | UpdateCorpusFileSignals | — | All scope signals for discovered inode; LibraryLeftoverSignal by path key | Automated cleanup of orphaned library files during deploy |
| UpdateTrackPath | UpdateCorpusFileSignals × 2 | — | (signals for both paths wiped) | Update path in index |
| Transcode | UpdateCorpusFileSignals × 2 | WaveformReadError | (signals for both paths wiped) | Transcode to new format |
| InboxToCorpus | (via dirty inodes) | — | MutableOnly scope signals for inode | Move inbox file to corpus; updates zone from inbox→corpus, migrates inbox_tags→corpus_tags |
| InboxDirToCorpus | (via dirty inodes) | — | MutableOnly scope signals for all tracked inodes | Move entire inbox directory to corpus via fs::rename; updates zone + migrates tags for each tracked audio file. Non-audio content (covers, booklets) travels with the directory. |

### Deployment Operations

| Mutation | Spawns Computations | Signals Emitted | Signals Cleared | Notes |
|----------|---------------------|-----------------|-----------------|-------|
| HardLink | UpdateDeploySignals | — | DeployReady | Deploy to library. Also deploys sidecar cover images alongside audio files (hard-links image files from corpus to library directory) |
| LibraryMove | — | — | LibraryStale | Library-internal move |

### OOB Resolution Operations

| Mutation | Spawns Computations | Signals Emitted | Signals Cleared | Notes |
|----------|---------------------|-----------------|-----------------|-------|
| AcknowledgeMtimeOnly | UpdateCorpusFileSignals | — | MtimeOnlyMismatch, MutableOnly scope signals for all track inodes | Update file mtime, acknowledge touch |
| AcknowledgeInodeChanged | UpdateCorpusFileSignals | — | InodeChanged | Update tracks.inode and files table for replaced files |

Note: ApplyDbTagsToDisk and AssimilateDiskTagsToDb are now single-track mutations documented in Tag Operations above. They clear OutOfBandTagSync, OutOfBandTagConflict, and tag_mismatch signals.

### External Match Operations

| Mutation | Spawns Computations | Signals Emitted | Signals Cleared | Notes |
|----------|---------------------|-----------------|-----------------|-------|
| DropExternalMatch | — | — | — | Delete external match data for an inode. `is_db_only: true`, `signal_clear_scope: None`, `affected_inodes: [inode]`. Triggers dirty inode marking for recomputation. |

### Signal Emission Operations

| Mutation | Spawns Computations | Signals Emitted | Signals Cleared | Notes |
|----------|---------------------|-----------------|-----------------|-------|
| EmitCanonicalTag | — | CanonicalTag | — | Whitelist a tag value as canonical (not compound) |
| EmitExpectedOverlap | — | ExpectedOverlap | CrossSourceOverlap (matching pair key) | Whitelist a source directory pair as expected overlap |
| EmitExpectedDuplicate | — | ExpectedDuplicate | RedundantDuplicate (matching fingerprint key) | Whitelist a fingerprint overlap group as expected duplicate |
| EmitExpectedMissingTag | — | ExpectedMissingTag | — | Whitelist inodes as having expected missing tags (persistent suppression). `is_db_only: true` |

The EmitCanonicalTag mutation is used when an operator confirms that a compound-looking value (e.g., "Rinse & Repeat") is actually a single canonical entity (band name) and should not be split. It emits a CanonicalTag signal with key `{tag_name}:{tag_value}` (whitelist entry). Future compound detection runs check for CanonicalTag signals and skip whitelisted values.

The EmitExpectedOverlap mutation is used when an operator confirms that cross-source overlap between two source directories is expected (e.g., tracks appearing on both original releases and game soundtracks). It emits an ExpectedOverlap signal with key `"source_a|source_b"` (sorted), then clears the corresponding CrossSourceOverlap signal. Future DetectCrossSourceOverlaps runs check for ExpectedOverlap signals and skip whitelisted pairs.

The EmitExpectedDuplicate mutation is used when an operator confirms that a fingerprint overlap group is expected (e.g., different tracks that legitimately sound nearly identical, like "Act Clear" vs "Act Clear (Silver or Bronze Medal)" on a game soundtrack). It emits an ExpectedDuplicate signal with the fingerprint key, then clears the corresponding RedundantDuplicate signal. Future AnalyzeFingerprintOverlaps runs check for ExpectedDuplicate signals and skip the entire fingerprint group (suppressing both RedundantDuplicate and SubparDuplicate emission).

The EmitExpectedMissingTag mutation is used when an operator confirms that certain inodes are expected to have missing tags (e.g., instrumental tracks intentionally lacking an ALBUM tag). It takes `inodes: Vec<i64>` and writes an ExpectedMissingTag corpus signal for each inode. `is_db_only: true`, `signal_clear_scope: None`, `affected_inode: None`. It does not spawn follow-up computations. Future DetectMissingTags runs check for ExpectedMissingTag signals and suppress those inodes from MissingAlbumSingleSignal emission.

### Config Operations

| Mutation | Spawns Mutations | Spawns Computations | Signals Emitted | Signals Cleared | Notes |
|----------|------------------|---------------------|-----------------|-----------------|-------|
| ApplyConfigEdits | — | — | — | — | Writes edited config to disk via comment-preserving KDL modification. Returns new Config in `TaskResult.config_update` for in-memory update via `Witch::update_shared_config()` |
| ApplyDirConfigEdit | — | — | — | — | Writes edited source directory config to dirs.kdl. Replaces a single SourceDir entry by matching on path. Fields: libraries, can_stash_dupes, interior_dupes, path_schema. Recomputation scope: FILES \| DEPLOY \| TAGS |
| ApplyBatchDirConfigEdits | — | — | — | — | Atomically applies multiple dir config edits to dirs.kdl in a single read-modify-write. Produced by coalescing multiple ApplyDirConfigEdit mutations at transaction commit time — never directly staged. Recomputation scope: union of per-entry scopes |

The ApplyConfigEdits mutation is created by the Config Editor view when the operator saves edited config fields. It carries the original KDL text, old config, and new config. On execution, it backs up `config.kdl` to `config.kdl.bak`, then applies field-level edits to the KDL document preserving comments and formatting. The new config is propagated back to the main thread via `TaskResult.config_update`, where `Witch::tick()` updates the `SharedConfig` (Arc<RwLock<Config>>). `is_db_only: false` (writes to filesystem), `signal_clear_scope: None`, `affected_inodes: empty`. No spawned computations or signal effects.

### Edit History Operations

| Mutation | Stage | Origin | Spawns | Signals | Notes |
|----------|-------|--------|--------|---------|-------|
| ExportEditHistory | DiskFlush | Staged | ClearEditHistory | — | Queries edit history rows via read-only DB, writes them to a timestamped TSV log file. On success, chain-emits ClearEditHistory. Two modes: single-session (`session_id: Some(id)`) or all-sessions (`session_id: None`). |
| ClearEditHistory | DB | ChainEmitted | — | — | Deletes edit history rows from the database. Only spawned by ExportEditHistory after successful export. Delegates to `write_thread::clear_tag_edit_history[_session]()`. |

Jettison goes through the standard transaction system with a witnessed decision (`DecisionKey::JettisonEditHistory`). The TUI's multi-phase confirmation UI (d/D shortcuts) gates the staging. The two-mutation chain ensures the export log is written before any rows are deleted — if the export fails, the chain-emission never happens and the database is untouched.

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

#### Disc Extraction TagOps

DiscExtraction resolution generates TagOps per inode depending on the extraction source:

- **Album source**: `TagOp::replace_tag(inode, "ALBUM", original, cleaned)` + `TagOp::add_tag(inode, disc_tag_name, disc_number)` — replaces the album tag with the cleaned version (disc suffix stripped) and adds a DISCNUMBER tag.
- **TrackNumber source**: `TagOp::replace_tag(inode, "TRACKNUMBER", original, cleaned_digits)` + `TagOp::add_tag(inode, disc_tag_name, disc_value)` — replaces the track number with the numeric portion only and adds a disc tag for the prefix.

The `disc_tag_name` is configurable via `DiscExtractionOpinions` (defaults to `"DISCNUMBER"`).

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
1. Spawn derivation-phase computations (`UpdateCorpusFileSignals` or `UpdateLibraryFileSignals`)
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
