# Mutation System Cleanup Plan

All corpus and index changes must flow through the Mutation system via TaskDaemon. This document identifies code that bypasses this architecture and needs cleanup.

## TaskDaemon Work Types: Mutations vs Computations

TaskDaemon handles two categories of work:

```
TaskDaemon
├── Mutations (require user decision)
│   └── Changes to corpus or index state
│
└── Computations (decisionless)
    └── Derived facts from corpus + index state
```

**Mutations** are state-altering operations that require explicit operator decisions. They change the corpus (files on disk) or index (database records). Examples: moving files, editing tags, dropping tracks from index.

**Computations** are derived facts computed from corpus and index state. They do not require user decisions and can be triggered automatically by:
- Heartbeat scans detecting filesystem changes
- Post-mutation side effects (e.g., after a tag edit, recompute tag health)
- Explicit "recompute" actions (e.g., "verify all tags from disk")

Both dispatch through TaskDaemon for parallel execution and progress tracking, but Computations bypass the decision requirement.

---

## Current Mutation Categories

```
Mutation
├── Corpus Mutations (disk changes)
│   ├── TagFlushToDisk      - Write tags to audio file
│   ├── Move / Copy         - File operations
│   ├── Delete / MoveToStash
│   └── HardLink / Unlink   - Deployment links
│
├── Index Mutations (database changes)
│   ├── TagEditDb           - Update tag in tracks table only
│   ├── IndexTrack          - Insert new track
│   ├── DropFromIndex       - Remove track from index
│   ├── UpdateTrackPath     - Fix relocated file path
│   ├── UpdateScanStatePath - Fix scan state for relocated file
│   ├── UpdateScanState     - Update mtime/inode tracking
│   ├── CleanupStaleScanState
│   └── UpdateTrack         - Full track metadata update
│
├── Combined Mutations (corpus + index)
│   └── TagEditAndFlush     - Update DB AND write to disk atomically
│
└── Administrative
    └── DbMigration         - Schema migrations
```

---

## Computation Categories

Computations produce Health Signals - facts about corpus health that power the Insights system. Unlike Mutations, they can execute without user decisions.

```
Computation
├── Verification (read-only fact gathering)
│   ├── VerifyTags          - Compare disk tags to index, emit signals
│   ├── VerifyFingerprint   - Re-fingerprint file, compare to index
│   └── VerifyFileExists    - Check file at path exists
│
├── Detection (pattern matching over index)
│   ├── DetectDuplicates    - Find fingerprint matches
│   ├── DetectTagConflicts  - Find inconsistent metadata
│   └── DetectOrphans       - Find files not in index
│
└── Signal Management
    ├── CreateHealthSignal  - Record detected condition
    └── ResolveHealthSignal - Mark condition resolved
```

**Key insight**: Health signals are just signals - they may or may not indicate problems. "OutOfBandTagChange" is a signal, not necessarily an issue. The Insights system interprets signals and presents actionable information to the operator.

**Trigger points for Computations**:
- Startup heartbeat (paranoid tag verification)
- Post-mutation hooks (after TagEditAndFlush, verify the result)
- Filesystem change detection (new/modified files)
- Explicit operator action ("Recompute all health signals")

---

## Non-Mutation Code That Needs Cleanup

### 1. `src/corpus/metadata.rs`

**`write_tags()` (lines 490-597)** - CRITICAL
- Currently: Writes to disk, then directly updates DB via `db.update_track_tag()`
- Problem: DB update bypasses mutation system, can fail silently
- Fix: Remove all DB operations (lines 576-594). This function should ONLY write to disk.
- The mutation executor (`TagEditAndFlush`) should handle DB updates separately.

**`write_artist_tag()` (lines 601-635)**
- Currently: Writes only to disk (OK)
- Problem: Doesn't update DB at all, causing desync
- Fix: Callers should use `TagEditAndFlush` mutation instead of calling this directly

### 2. `src/ui/mod.rs` line 1697

```rust
if let Err(e) = metadata::write_tags(&file.path, &new_tags, 0, "") {
```

- Context: Directory tag editor save flow
- Problem: Direct call bypasses mutation system
- Fix: Replace with `todo!("Queue TagEditAndFlush mutations to TaskDaemon")`

### 3. `src/flows/changes.rs`

**Line 146, 325:**
```rust
match db.delete_track_by_path(&decision.source_path) {
```
- Problem: Direct DB delete during decision execution
- Fix: Should create `DropFromIndex` mutation

**Line 214:**
```rust
crate::corpus::metadata::write_tags(path, &tags, track_id, "")
```
- Problem: Direct write bypasses mutation system
- Fix: Should create `TagEditAndFlush` mutation

### 4. `src/corpus/health/heartbeat.rs` line 608

```rust
if db.insert_track(&track).is_err() {
```

- Context: Auto-indexing new files during heartbeat
- Problem: Direct DB insert bypasses mutation system
- Decision needed: Should heartbeat auto-index, or just detect and signal?
- Current behavior conflicts with "MLA never makes Decisions autonomously" principle
- Fix: Remove auto-indexing. Heartbeat should emit a `NewFileDetected` signal instead.
  The operator can then choose to index via the Insights view.

### 5. `src/corpus/mutations/tag_edit.rs` line 130

```rust
metadata::write_tags(path, &final_tags, track_id, session_id)
```

- Context: Inside `execute_tag_edit_and_flush()`
- Problem: Uses `write_tags()` which does its own DB update
- Fix: Call disk-only write, then do DB update in executor

---

## Cleanup Steps

### Phase 1: Fix metadata.rs ✅ COMPLETE

1. ✅ Renamed `write_tags()` to `write_tags_to_file()`
2. ✅ Removed all DB operations from it (lines 576-594)
3. ✅ Kept only the file writing logic
4. ✅ Added `MutationToken` requirement for corpus-mutating functions

### Phase 2: Fix mutation executors ✅ COMPLETE

1. ✅ Updated `tag_edit.rs:execute_combined()`:
   - Creates `MutationToken` internally
   - Calls `write_tags_to_file(token)` for disk write
   - Calls `db.update_track_tag()` directly for DB update
   - Handles errors properly

### Phase 3: Add DecisionWitness to TaskDaemon ✅ COMPLETE

Added two-layer witness architecture:

```
User Decision → DecisionWitness → TaskDaemon → MutationToken → Corpus Change
```

- ✅ `DecisionWitness` in `flows/daemon.rs` - proves mutations come from user-led Decision context
- ✅ `confirm_decision()` helper - centralizes the decision point in UI code
- ✅ Updated `queue()`, `queue_all()` etc. to require witness
- ✅ Added `queue_computation()`, `queue_computations()` (no witness required)

### Phase 4: Formalize Computations ✅ COMPLETE

1. ✅ Created `Computation` enum in `src/corpus/computations/mod.rs`
2. ✅ Moved `VerifyTags` from `Mutation` to `Computation`
3. ✅ Added computation executor in `computations::execute_single()`
4. ✅ Extended TaskDaemon to handle both `Task::Mutation` and `Task::Computation`
5. ✅ Updated tag verification flow to use Computation type

### Phase 5: Stub non-mutation callers ✅ COMPLETE

All bypass points are stubbed with `todo!()`:

```rust
// src/ui/mod.rs - Tag editor save (needs DecisionWitness)
todo!("Reconnect: Tag editor save needs DecisionWitness");

// src/ui/mod.rs - Deployment confirm (needs DecisionWitness)
todo!("Reconnect: Deployment confirm needs DecisionWitness");

// src/ui/mod.rs - Directory tag editor save
todo!("Refactor: Queue TagEditAndFlush mutations to TaskDaemon");

// src/flows/changes.rs - Delete handler
todo!("Refactor: Create MoveToStash + DropFromIndex mutations via TaskDaemon")

// src/flows/changes.rs - DropIndex handler
todo!("Refactor: Create DropFromIndex mutation via TaskDaemon")
```

---

## Follow-Up Work (UI Integration)

The following tasks remain for full UI integration:

1. **Wire tag editor save** - Create `DecisionWitness` and queue `TagEditAndFlush` mutations
2. **Wire deployment confirm** - Create `DecisionWitness` and queue deployment mutations
3. **Wire directory tag editor** - Queue `TagEditAndFlush` mutations via TaskDaemon
4. **Remove flows/changes.rs bypass** - Replace decision execution with proper mutations

---

## Architecture Diagram

```
┌─────────────────────────────────────────────────────────────────┐
│                        TaskDaemon                               │
│  ┌─────────────────────┐    ┌─────────────────────────────┐    │
│  │     Mutations       │    │       Computations          │    │
│  │  (require decision) │    │     (auto-executable)       │    │
│  └──────────┬──────────┘    └──────────────┬──────────────┘    │
└─────────────┼──────────────────────────────┼───────────────────┘
              │                              │
              v                              v
┌─────────────────────────┐    ┌─────────────────────────────────┐
│   Corpus (disk files)   │    │      Health Signals Table       │
│   Index (tracks table)  │    │   (tag_mismatches, health_     │
│                         │    │    issues, etc.)                │
└─────────────────────────┘    └─────────────────────────────────┘
              │                              │
              └──────────────┬───────────────┘
                             v
                   ┌─────────────────────┐
                   │   Insights System   │
                   │ (real-time queries) │
                   └─────────────────────┘
                             │
                             v
                   ┌─────────────────────┐
                   │     Operator UI     │
                   └─────────────────────┘
```

---

## Files Summary

| File | Line(s) | Issue | Action |
|------|---------|-------|--------|
| `corpus/metadata.rs` | 576-594 | DB write in write_tags() | Remove DB code |
| `corpus/metadata.rs` | 601-635 | write_artist_tag() no DB | Deprecate |
| `ui/mod.rs` | 1697 | Direct write_tags call | Stub with todo!() |
| `flows/changes.rs` | 146, 325 | Direct delete_track_by_path | Stub with todo!() |
| `flows/changes.rs` | 214 | Direct write_tags call | Stub with todo!() |
| `health/heartbeat.rs` | 608 | Direct insert_track | Remove, emit signal |
| `mutations/tag_edit.rs` | 130 | Uses DB-writing write_tags | Use disk-only version |
