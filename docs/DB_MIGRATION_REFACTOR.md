# Database Migration Subsystem Refactor

This document outlines a refactor of MLA's database migration system to achieve proper separation between corpus index tables (mutation-only) and computed health signal tables, with strong safety guarantees.

## Current State

### Migration System (`corpus/mutations/migration.rs`)

The current migration system:
- Uses a `MigrationRegistry` with versioned migrations (v2 → v3 → v4)
- Runs automatically at startup with a progress popup
- Does NOT require explicit operator approval
- Is NOT integrated with the mutations system (bypasses `DecisionWitness` entirely)
- Operates on a single monolithic database file (`mla.db`)

### Tables in Current Schema

| Category | Tables | Mutation/Computation |
|----------|--------|---------------------|
| **Corpus Index** | `tracks`, `track_tags`, `scan_state`, `scan_history` | Mutation-only |
| **Health Signals** | `health_issues`, `health_issue_tracks`, `known_variants`, `tag_canonicalization`, `tag_mismatches`, `corpus_health_stats` | Computation-derived |
| **Mutation Audit** | `tag_edit_history`, `deployment_log` | Mutation-only |
| **Admin** | `app_metadata` | Mixed |

### Problems

1. **No operator approval**: Migrations run without explicit confirmation, violating "MLA never makes Decisions autonomously"
2. **No separation of concerns**: Mutation-only tables live alongside computation tables in one file
3. **Version sprawl**: We're at v4 with legacy migrations from early development
4. **Mixed table concerns**: Health signals (derived data) can be safely dropped/rebuilt, but are versioned alongside critical index data
5. **Bypass points**: Multiple code paths can write to index tables without going through TaskDaemon

---

## Safety Architecture: Witness Chain

### The Problem

MLA's core invariant is that all corpus/index mutations must trace back to explicit operator decisions. Currently, this is enforced at the TaskDaemon queue level via `DecisionWitness`, but:

1. Mutation executors have direct access to `db.insert_track()`, `db.update_track_tag()`, etc.
2. Code outside the mutation system could theoretically call these functions
3. The migration system completely bypasses this protection

### The Solution: Two-Level Witness Chain

```
User Decision (Enter key press)
        │
        ▼
DecisionWitness (proves user confirmed)
        │
        ▼
TaskDaemon.queue()
        │
        ▼
[worker thread executes mutation]
        │
        ▼
MutationExecutionWitness (proves execution is inside daemon)
        │
        ▼
db.insert_track(), db.update_track_tag(), etc.
```

**`DecisionWitness`** (already exists): Proves mutations are being queued from a user-led Decision context. Only obtainable via `confirm_decision()` which should be called when user presses Enter to confirm.

**`MutationExecutionWitness`** (new): Proves that index-mutating database functions are being called from within a TaskDaemon mutation executor. Only constructible inside the daemon's worker thread execution path.

### Index-Mutating Functions Requiring Witness

All functions that write to `index.db` tables must require a `MutationExecutionWitness`:

| Module | Function | Table Affected |
|--------|----------|----------------|
| `db/queries/tracks.rs` | `insert_track()` | tracks |
| `db/queries/tracks.rs` | `update_track_tag()` | tracks |
| `db/queries/tracks.rs` | `update_track_path()` | tracks |
| `db/queries/tracks.rs` | `update_track_metadata()` | tracks |
| `db/queries/tracks.rs` | `delete_track()` | tracks |
| `db/queries/tracks.rs` | `delete_track_by_path()` | tracks |
| `db/queries/tracks.rs` | `delete_tracks_by_paths()` | tracks |
| `db/queries/tracks.rs` | `record_tag_edit()` | tag_edit_history |
| `db/queries/scan_state.rs` | `upsert_scan_state()` | scan_state |
| `db/queries/scan_state.rs` | `delete_scan_state_by_inode()` | scan_state |
| `db/queries/scan_state.rs` | `update_scan_state_path()` | scan_state |
| `db/queries/deployment.rs` | `record_deployment()` | deployment_log |
| `corpus/metadata.rs` | `write_tags_to_file()` | (disk, already has MutationToken) |

### Signal-Writing Functions (No Witness Required)

These write to `signals.db` and are computation-derived, so no witness needed:

| Module | Function | Table Affected |
|--------|----------|----------------|
| `db/queries/health.rs` | `insert_health_issue()` | health_issues |
| `db/queries/health.rs` | `link_track_to_issue()` | health_issue_tracks |
| `db/queries/health.rs` | `resolve_health_issue()` | health_issues |
| `db/queries/health.rs` | `insert_known_variant()` | known_variants |
| `db/queries/metadata.rs` | `upsert_tag_canonicalization()` | tag_canonicalization |
| `db/queries/metadata.rs` | `insert_tag_mismatch()` | tag_mismatches |

---

## Code Audit: Bypass Points

The following code paths currently write to index tables without going through the witness chain. These must be addressed:

### Already Stubbed with `todo!()`

| File | Line | Issue | Status |
|------|------|-------|--------|
| `flows/changes.rs` | 124 | Delete decision bypasses TaskDaemon | Stubbed |
| `flows/changes.rs` | 176 | TagEdit decision bypasses TaskDaemon | Stubbed |
| `flows/changes.rs` | 283 | DropIndex decision bypasses TaskDaemon | Stubbed |
| `ui/mod.rs` | ~1044 | Tag editor save | Stubbed |
| `ui/mod.rs` | ~1165 | Deployment confirm | Stubbed |

### Mutation Executors (Correct Path)

These are the **intended** code paths that execute inside TaskDaemon:

| File | Function | Notes |
|------|----------|-------|
| `mutations/indexing.rs` | `execute_index_track()` | Calls `db.insert_track()` |
| `mutations/indexing.rs` | `execute_update_track_path()` | Calls `db.update_track_path()` |
| `mutations/indexing.rs` | `execute_drop_from_index()` | Calls `db.delete_track()` |
| `mutations/indexing.rs` | `execute_update_track()` | Calls `db.update_track_metadata()` |
| `mutations/tag_edit.rs` | `execute_tag_edit_db_only()` | Calls `db.update_track_tag()` |
| `mutations/tag_edit.rs` | `execute_combined()` | Calls `write_tags_to_file()`, `db.update_track_tag()` |
| `mutations/file_ops.rs` | `execute_stash()` | Calls `db.delete_track_by_path()` |

### Potential Hidden Bypass Points

After auditing, these are areas to verify don't have hidden writes:

| Area | Risk | Action |
|------|------|--------|
| `corpus/health/detection.rs` | Calls `insert_health_issue()` | OK - signals only |
| `corpus/health/canonicalization.rs` | Calls `upsert_tag_canonicalization()` | OK - signals only |
| `corpus/computations/mod.rs` | Calls `insert_health_issue()` | OK - signals only |
| `corpus/eyeballing.rs` | Read-only scanning | OK - no writes |

---

## Design Goals

1. **Separate databases**: Split into `index.db` (corpus index) and `signals.db` (health signals/computations)
2. **Freeze index schema**: Lock `index.db` tables at version 1; changes require explicit migration
3. **Ephemeral signals**: `signals.db` can always be dropped and recomputed from `index.db`
4. **Require approval**: Add startup confirmation popup for any index migrations
5. **Clean slate**: Since operator can rm the db file, no backwards migration path needed
6. **Strong witnesses**: All index writes require `MutationExecutionWitness`

---

## Proposed Architecture

### Database Files

```
$XDG_DATA_HOME/mla/
├── index.db       # Corpus index - mutation-only, versioned
└── signals.db     # Health signals - computation-derived, ephemeral
```

### index.db (Corpus Index)

Tables:
- `tracks` - Core track metadata and fingerprints
- `track_tags` - Arbitrary tag key-value pairs
- `scan_state` - Incremental scan optimization (inode+mtime tracking)
- `scan_history` - Completed scan log
- `deployment_log` - Library deployment records
- `tag_edit_history` - Mutation audit trail
- `db_meta` - Schema version (starts at 1), app version

**Schema version**: 1 (frozen)

**Future changes**: Any schema change to `index.db` requires:
1. A new migration entry
2. Operator approval popup at startup
3. Consideration of data preservation (though currently rm-and-rebuild is acceptable)

### signals.db (Health Signals)

Tables:
- `health_issues` - Detected corpus health problems
- `health_issue_tracks` - Links tracks to issues (references track paths, not IDs)
- `known_variants` - Accepted duplicates
- `tag_canonicalization` - Tag normalization mappings
- `tag_mismatches` - DB vs disk tag differences
- `corpus_health_stats` - Cached aggregate statistics
- `signal_meta` - Computation version, last recompute timestamp

**Schema version**: Not versioned. Schema is defined in code; mismatch triggers drop + rebuild.

**Rebuild trigger**: If `signals.db` schema doesn't match expected, or signal computation version changes, silently drop and recompute. This is safe because all data derives from `index.db` + filesystem.

### Cross-Database References

Health signals reference tracks from `index.db`. To fully decouple:

**Store `track_path` instead of `track_id`**:
- `health_issue_tracks.track_path TEXT` instead of `track_id INTEGER`
- Join via `path` when needed
- Signals remain valid even if track IDs change
- No foreign key constraints across databases

---

## TaskDaemon: Three Task Types

### Current State

```rust
pub enum Task {
    Mutation(Mutation),    // Requires DecisionWitness
    Computation(Computation), // No witness required
}
```

### Proposed State

```rust
pub enum Task {
    Mutation(Mutation),       // Requires DecisionWitness, blocked until accepting_mutations
    Computation(Computation), // No witness required, always allowed
    Migration(Migration),     // Requires DecisionWitness, NOT blocked by accepting_mutations
}
```

### Migration Task Type

Migrations are a special case:
- **Require `DecisionWitness`**: User must approve migrations (not automatic)
- **Not blocked by observation state**: Can run before corpus eyeballing completes
- **Limited scope**: Only one-time schema migrations, no corpus data changes
- **Run at startup**: Before main event loop begins

```rust
pub enum Migration {
    IndexSchemaMigration {
        from_version: u32,
        to_version: u32,
        description: String,
    },
}
```

### Startup Flow

```
1. Open/create index.db
2. Check index.db schema version
   ├── Version matches current (1): proceed to step 3
   ├── Version < current: show migration approval popup
   │   ├── User approves (Enter):
   │   │   └── Create DecisionWitness, queue Migration tasks
   │   │   └── Wait for migrations to complete
   │   │   └── Proceed to step 3
   │   └── User declines (Esc): exit application
   └── Version > current: error (newer DB than app)

3. Open/create signals.db
4. Check signals.db schema
   ├── Schema matches expected: proceed
   └── Schema mismatch: drop all tables, recreate schema (no approval needed)

5. Check signal computation version
   ├── Version matches: proceed
   └── Version mismatch: schedule background recompute

6. Start paranoid eyeballing (queues Computation tasks)
7. Continue to main UI (eye closed until eyeballing completes)
```

### Why Migrations Need DecisionWitness But Not Observation Gate

The `accepting_mutations` flag gates mutations that modify corpus data because we need to verify the corpus state first. But migrations:

1. Don't modify corpus data - they only modify schema
2. Must run BEFORE indexing can happen (new schema may be required)
3. Are one-time operations that fundamentally change capabilities

The user approval at startup serves as the Decision for migrations. Once approved, they execute immediately without waiting for eyeballing.

---

## Witness Implementation

### MutationExecutionWitness

```rust
// In corpus/mutations/mod.rs

pub mod execution_sealed {
    /// Proof that code is executing inside TaskDaemon's mutation worker.
    ///
    /// Cannot be constructed outside the daemon's execute_mutation() function.
    /// All index-mutating database functions require this witness.
    #[derive(Clone, Copy)]
    pub struct MutationExecutionWitness(());

    impl MutationExecutionWitness {
        /// Internal constructor - only callable from daemon execution context.
        pub(in crate::daemon) fn new() -> Self {
            Self(())
        }
    }
}

pub use execution_sealed::MutationExecutionWitness;
```

### Database Functions With Witness

```rust
// In corpus/db/queries/tracks.rs

impl Database {
    /// Insert a new track into the index.
    ///
    /// Requires `MutationExecutionWitness` to prove this is called from
    /// TaskDaemon's mutation execution context.
    pub fn insert_track(
        &self,
        track: &Track,
        _witness: &MutationExecutionWitness
    ) -> Result<i64> {
        // ... existing implementation
    }
}
```

### Daemon Creates Witness

```rust
// In daemon.rs

fn execute_mutation(mutation: Mutation, label: String) -> TaskResult {
    // Create witness - only possible inside daemon worker
    let witness = MutationExecutionWitness::new();

    // Open database
    let db = /* ... */;

    // Pass witness to mutation executors
    let result = match mutation.category() {
        MutationCategory::TagEdit => {
            tag_edit::execute_single(&db, &mutation, session_id, &witness)
        }
        MutationCategory::Indexing => {
            indexing::execute_single(&db, &mutation, &witness)
        }
        // ...
    };

    TaskResult { /* ... */ }
}
```

### Mutation Executors Pass Witness

```rust
// In corpus/mutations/indexing.rs

pub fn execute_single(
    db: &Database,
    mutation: &Mutation,
    witness: &MutationExecutionWitness,
) -> MutationResult {
    match mutation {
        Mutation::IndexTrack { path, source, metadata } => {
            execute_index_track(db, path, source, metadata, witness)
        }
        // ...
    }
}

fn execute_index_track(
    db: &Database,
    path: &Path,
    source: &str,
    metadata: &TrackMetadata,
    witness: &MutationExecutionWitness,
) -> Result<()> {
    let track = /* build track */;
    db.insert_track(&track, witness)?; // Witness required
    Ok(())
}
```

---

## Migration Approval Popup

```
┌─────────────────────────────────────────────────────┐
│           Database Migration Required               │
├─────────────────────────────────────────────────────┤
│                                                     │
│  MLA needs to upgrade your corpus index database.   │
│                                                     │
│  Current version: 1                                 │
│  Target version:  2                                 │
│                                                     │
│  Pending migrations:                                │
│    • v1 → v2: Add track_tags table                 │
│                                                     │
│  ⚠ This cannot be interrupted once started.        │
│                                                     │
│  [Enter] Proceed    [Esc] Cancel                   │
└─────────────────────────────────────────────────────┘
```

If user cancels: exit application cleanly.

---

## Implementation Plan

### Phase 1: Add MutationExecutionWitness

1. Create `execution_sealed` module in `corpus/mutations/mod.rs`
2. Add `MutationExecutionWitness` type
3. Update `daemon.rs:execute_mutation()` to create witness
4. Update all mutation executor signatures to accept witness
5. Update all index-mutating `Database` methods to require witness

**Files changed:**
- `corpus/mutations/mod.rs` - Add witness type
- `daemon.rs` - Create witness in execute_mutation
- `corpus/mutations/indexing.rs` - Accept and pass witness
- `corpus/mutations/tag_edit.rs` - Accept and pass witness
- `corpus/mutations/file_ops.rs` - Accept and pass witness
- `corpus/db/queries/tracks.rs` - Require witness
- `corpus/db/queries/scan_state.rs` - Require witness
- `corpus/db/queries/deployment.rs` - Require witness

### Phase 2: Add Migration Task Type

1. Add `Migration` enum to `daemon.rs`
2. Update `Task` enum to include `Migration` variant
3. Add `queue_migration()` method requiring `DecisionWitness`
4. Migrations bypass `accepting_mutations` check
5. Create `execute_migration()` function

**Files changed:**
- `daemon.rs` - Add Migration type and queueing
- `corpus/mutations/migration.rs` - Adapt for new system

### Phase 3: Update Startup Flow

1. Modify `ui/mod.rs` migration popup to require explicit approval
2. Add "Cancel" option that exits application
3. Create `DecisionWitness` on user approval
4. Queue migrations through TaskDaemon
5. Wait for migrations before proceeding

**Files changed:**
- `ui/mod.rs` - Update migration popup flow

### Phase 4: Create Two-Database Structure

1. Create `corpus/db/index_db.rs` with IndexDb wrapper
2. Create `corpus/db/signals_db.rs` with SignalsDb wrapper
3. Move table schemas to appropriate modules
4. Update `health_issue_tracks` to use `track_path` instead of `track_id`
5. Create `Databases` struct holding both connections

**Files changed:**
- `corpus/db/mod.rs` - Restructure exports
- `corpus/db/index_db.rs` - NEW
- `corpus/db/signals_db.rs` - NEW
- `corpus/db/databases.rs` - NEW
- `corpus/db/queries/*` - Route to correct db

### Phase 5: Update Query Modules

1. Update `tracks.rs` to use IndexDb
2. Update `scan_state.rs` to use IndexDb
3. Update `deployment.rs` to use IndexDb
4. Update `health.rs` to use SignalsDb
5. Update `metadata.rs` to split between both
6. Update all call sites that query health issues to join via path

**Files changed:**
- All files in `corpus/db/queries/`
- All callers of health issue queries

### Phase 6: Implement Signal Schema Validation

1. Add schema fingerprint check to `SignalsDb::open()`
2. Compare expected table definitions to actual
3. On mismatch: `recreate_schema()`
4. Add computation version check
5. If version mismatch: schedule recompute in TaskDaemon

**Files changed:**
- `corpus/db/signals_db.rs`
- `config.rs` - Add separate path getters

### Phase 7: Clean Up Migration Registry

1. Reset index.db schema version to 1
2. Remove legacy migrations (v2→v3, v3→v4)
3. Squash current schema into baseline v1
4. Simplify MigrationRegistry for forward-only migrations

**Files changed:**
- `corpus/mutations/migration.rs` - Simplify
- `corpus/db/queries/mod.rs` - Update baseline schema

---

## Deferred Concerns

### Read-Only Observation Gate for Migrations

The current constraint:
- TaskDaemon starts with `accepting_mutations = false`
- Mutations are rejected until eyeballing completes
- But migrations might need to run BEFORE eyeballing

**Solution**: Migrations are a separate task type that bypasses `accepting_mutations`. They require `DecisionWitness` (user approval) but not observation completion. This is safe because:
1. Migrations don't modify corpus data
2. User explicitly approved the migration
3. Migrations are schema-only changes

If we ever need to do data migrations (e.g., populating a new column from existing data), we would need to:
1. Run schema migration first (new column)
2. Wait for eyeballing to complete
3. Run data population as a normal Mutation batch

### Multiple Connection Performance

SQLite with two database files means two connections. For our use case (local tool, single user), this is fine. If performance becomes an issue:
- Use connection pooling
- Consider WAL mode for both databases
- Profile actual query patterns

### Join Performance Across Databases

Joining `health_issue_tracks.track_path` to `tracks.path` requires:
- Loading track paths from signals, then querying index by path
- Or using ATTACH (shares write lock)

Current approach: Query by path. Defer ATTACH until proven necessary.

---

## Schema Definitions

### index.db Schema (Version 1)

```sql
-- Schema metadata
CREATE TABLE db_meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
-- INSERT INTO db_meta (key, value) VALUES ('schema_version', '1');

-- Core track metadata
CREATE TABLE tracks (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL UNIQUE,
    source TEXT NOT NULL,
    inode INTEGER NOT NULL,
    file_size INTEGER NOT NULL,
    file_type TEXT NOT NULL,
    artist TEXT,
    album TEXT,
    album_artist TEXT,
    title TEXT,
    track_number INTEGER,
    genre TEXT,
    duration_ms INTEGER,
    bitrate_kbps INTEGER,
    sample_rate INTEGER,
    fingerprint TEXT,
    isrc TEXT,
    scanned_at DATETIME DEFAULT CURRENT_TIMESTAMP
);
-- Indexes: source, inode, artist, album, album_artist, title,
-- duration_ms, fingerprint, genre, isrc, LOWER variants

-- Arbitrary tag storage
CREATE TABLE track_tags (
    id INTEGER PRIMARY KEY,
    track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    tag_name TEXT NOT NULL,
    tag_value TEXT NOT NULL,
    source TEXT NOT NULL DEFAULT 'disk',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(track_id, tag_name)
);

-- Incremental scan state
CREATE TABLE scan_state (
    id INTEGER PRIMARY KEY,
    source TEXT NOT NULL,
    inode INTEGER NOT NULL,
    path TEXT NOT NULL,
    mtime_secs INTEGER NOT NULL,
    mtime_nanos INTEGER NOT NULL,
    file_size INTEGER NOT NULL,
    scanned_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(source, inode)
);

-- Scan history
CREATE TABLE scan_history (
    id INTEGER PRIMARY KEY,
    source TEXT NOT NULL,
    file_count INTEGER NOT NULL,
    started_at DATETIME NOT NULL,
    completed_at DATETIME NOT NULL
);

-- Deployment log
CREATE TABLE deployment_log (
    id INTEGER PRIMARY KEY,
    library_name TEXT NOT NULL,
    corpus_path TEXT NOT NULL,
    deployed_path TEXT NOT NULL,
    inode INTEGER NOT NULL,
    deployed_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

-- Tag edit audit trail
CREATE TABLE tag_edit_history (
    id INTEGER PRIMARY KEY,
    track_id INTEGER NOT NULL,
    field_name TEXT NOT NULL,
    old_value TEXT,
    new_value TEXT,
    edited_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    session_id TEXT,
    FOREIGN KEY(track_id) REFERENCES tracks(id)
);
```

### signals.db Schema

```sql
-- Signal metadata
CREATE TABLE signal_meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Health issues (track_path instead of track_id for decoupling)
CREATE TABLE health_issues (
    id INTEGER PRIMARY KEY,
    issue_type TEXT NOT NULL,
    issue_key TEXT NOT NULL,
    severity TEXT NOT NULL,
    discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    resolved_at DATETIME,
    resolution_type TEXT,
    resolution_session TEXT,
    metadata_json TEXT
);

-- Links tracks to health issues via path (not foreign key)
CREATE TABLE health_issue_tracks (
    id INTEGER PRIMARY KEY,
    issue_id INTEGER NOT NULL REFERENCES health_issues(id) ON DELETE CASCADE,
    track_path TEXT NOT NULL,
    role TEXT NOT NULL
);

-- Known variants (paths instead of track IDs)
CREATE TABLE known_variants (
    id INTEGER PRIMARY KEY,
    variant_type TEXT NOT NULL,
    canonical_fingerprint TEXT NOT NULL,
    variant_fingerprint TEXT,
    canonical_track_path TEXT,
    variant_track_path TEXT,
    marked_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    notes TEXT
);

-- Tag canonicalization
CREATE TABLE tag_canonicalization (
    id INTEGER PRIMARY KEY,
    tag_name TEXT NOT NULL,
    canonical_value TEXT NOT NULL,
    variant_value TEXT NOT NULL,
    confidence REAL,
    auto_detected INTEGER DEFAULT 1,
    confirmed_at DATETIME,
    UNIQUE(tag_name, variant_value)
);

-- Tag mismatches (path instead of track_id)
CREATE TABLE tag_mismatches (
    id INTEGER PRIMARY KEY,
    track_path TEXT NOT NULL,
    field TEXT NOT NULL,
    db_value TEXT,
    disk_value TEXT,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(track_path, field)
);

-- Cached statistics
CREATE TABLE corpus_health_stats (
    id INTEGER PRIMARY KEY,
    stat_type TEXT NOT NULL,
    last_updated DATETIME DEFAULT CURRENT_TIMESTAMP,
    data_json TEXT NOT NULL
);
```

---

## File Changes Summary

| File | Action |
|------|--------|
| `corpus/mutations/mod.rs` | Add `MutationExecutionWitness` type |
| `daemon.rs` | Create witness in execute_mutation, add Migration task type |
| `corpus/mutations/indexing.rs` | Accept and pass witness to DB calls |
| `corpus/mutations/tag_edit.rs` | Accept and pass witness to DB calls |
| `corpus/mutations/file_ops.rs` | Accept and pass witness to DB calls |
| `corpus/db/queries/tracks.rs` | Require witness on mutating methods |
| `corpus/db/queries/scan_state.rs` | Require witness on mutating methods |
| `corpus/db/queries/deployment.rs` | Require witness on mutating methods |
| `corpus/db/mod.rs` | Update exports for two-database structure |
| `corpus/db/index_db.rs` | NEW - IndexDb connection wrapper |
| `corpus/db/signals_db.rs` | NEW - SignalsDb connection wrapper |
| `corpus/db/databases.rs` | NEW - Combined Databases struct |
| `corpus/db/queries/health.rs` | Use SignalsDb, change to path-based refs |
| `corpus/db/queries/metadata.rs` | Split between IndexDb and SignalsDb |
| `corpus/mutations/migration.rs` | Simplify, integrate with TaskDaemon |
| `ui/mod.rs` | Update migration popup with approval requirement |
| `config.rs` | Add separate path getters for index.db/signals.db |
| `CLAUDE.md` | Update data locations documentation |
| `docs/DATABASE.md` | Update schema documentation |
