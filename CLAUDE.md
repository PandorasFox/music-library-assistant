# Music Magic - Claude Development Notes

Check the docs/ folder for the Architecture overview and UI guidelines/UX consistency notes.

The Rust type system is our friend. Always try to encode information via types whenever possible, rather than attempting inferences on strings.

Do not spawn threads, ever. Only the Witch should spawn new threads for background work. That is *Her* purpose: She enforces orderliness in her domain, providing a consistent and clean way for doing background work, with controlled gates for their side effects. All data that flows properly through Her is guaranteed.

Do not try to "refresh" signals. MM is designed around precisely recomputing relevant signals in real-time. You keep adding unused (!) refresh hooks that then only get misused, because they're not things we need or want architecturally. They are expensive.

We use 0-byte Witness objects as guarantees for some compile-time guarantees about correctness and operational intents. Do not ever instantiate a Witness object - do a todo!() instead so that a panic happens and *I* can decide if a Witness is appropriately instantiable there, or not.

fs::remove_file (and similar logic that can potentially unlink inodes or free up block device storage) shall not be introduced to MM's codebase. Unlinking corpus files is solely operator privilege and is not to be conceptually introduced to MM, ever. [obsolete and empty source code files themselves can be deleted]

This is still alpha~beta software; breaking changes are encouraged - stop focusing on backwards-compatibility and 'legacy' compatibility.

### Database Access Patterns

MM enforces strict separation between read-only UI queries and write mutations:

**Read-Only Access (UI Code):**
- can use the Witch's exposed read-only DB connection, or the UiCache for expensive queries

**Write Access (Worker Threads Only):**
- only accessible via db_thread send channel, from within the Mutation (`meta::mutations`) and Computation (`meta::computations`) execution closures.
- worker threads also have their own thread-local read-only db connections to avoid read/write lock contention in bulk work.

There Shall Not be any other ways to interface with the DB. We have very nice read-only wrappers.

### Corpus vs Library File Queries

**CRITICAL: The `files` table contains BOTH corpus files AND library files.** They are distinguished by the `zone` column (`'corpus'` vs `'library'`). The `audio_info` table contains audio metadata for files from BOTH zones.

**Health detection queries MUST filter by `zone = 'corpus'`.** Computations like duplicate detection, missing tag detection, and overlap analysis should only operate on corpus files. Library files are deployment targets, not sources of truth.

**Correct pattern for corpus-only queries:**
```sql
-- When joining files with audio_info for health detection:
SELECT ... FROM files f
JOIN audio_info a ON f.inode = a.inode
WHERE f.zone = 'corpus' AND ...

-- When querying audio_info directly, JOIN with files to filter:
SELECT a.fingerprint, GROUP_CONCAT(a.inode)
FROM audio_info a
JOIN files f ON a.inode = f.inode
WHERE a.fingerprint IS NOT NULL AND f.zone = 'corpus'
GROUP BY a.fingerprint
```

**Anti-patterns:**
- Querying `audio_info` without joining `files` for zone filtering
- Assuming all files in `files` table are corpus files
- Using `files` queries without `WHERE zone = 'corpus'` in health detection

**When library files ARE needed:**
- `get_library_files()` - for deploy health checking
- `get_all_library_inodes()` - for checking what's deployed
- Queries explicitly about library state (leftovers, stale deployments)

### UI Caching Strategies

**Never query the database directly from render code.** DB queries during render cause multi-second frame times when workers are active, and create SQLite contention with write operations.

Two caching strategies exist for different use cases:

**1. UiCache (`ui/cache.rs`) - For "lively" data that updates during operation**

Use for data that changes while the user watches: corpus summary, Witch status, health stats.

```rust
// In event loop (ui/mod.rs run_app):
if let Some(ref mut the_witch) = app.witch {
    app.ui_cache.refresh(the_witch);  // Refreshes stale cached values
}

// In render code:
let summary = app.ui_cache.corpus_summary();  // Returns cached value, never queries DB
```

Adding new cached values to UiCache:
1. Add field to `UiCache` struct with `_at: Instant` timestamp
2. Add constant for refresh interval (e.g., `CORPUS_SUMMARY_INTERVAL`)
3. Add refresh logic in `refresh()` method
4. Add getter method that returns cloned/copied value

**2. Modal/View Init Caching - For data static during interaction**

Use for data that stays fixed while a modal or view is open: tag editor loading track tags, search results populating a list.

```rust
// Query once during modal/view initialization (caller uses read_db):
let read_db = witch.read_db();
let state = TagEditorState::new(track_id, read_db.inner());

impl TagEditorState {
    pub fn new(track_id: i64, db: &Database) -> Self {
        let tags = db.get_track_tags(track_id).unwrap_or_default();  // Query here, once
        Self {
            cached_tags: tags,  // Store in state struct
            // ...
        }
    }
}

// In render code - use the cached data:
fn render(&self, f: &mut Frame, area: Rect) {
    for tag in &self.cached_tags {  // Never query, just read cached
        // ...
    }
}
```

**Anti-patterns:**
- `witch.read_db().inner().get_*()` in render functions
- Passing `&Database` or `ReadOnlyDb` to render/display code
- Any DB query inside `render()` or functions it calls

### Widget-First UI Development

When building new UI components, **always use existing widgets first**. The `ui/widgets/` module provides reusable, composable components:

| Widget | Purpose |
|--------|---------|
| `SelectableList` | Navigable lists with selection highlighting |
| `HealthStatus` | Consistent status coloring (Healthy, Info, Warning, Critical) |
| `StatusIndicator` | Single-item status display with icon/label |
| `ThreePaneLayout` | Standard multi-pane layouts |
| `Modal`, `ConfirmationModal` | Popup dialogs |
| `ControlsHint` | Context-sensitive keyboard hints |

**Guidelines:**
1. Check `ui/widgets/` before creating new rendering code
2. If existing widgets don't fit, add to or extend them rather than creating ad-hoc rendering
3. Keep UI-agnostic logic (like severity levels) separate from widget styling
4. Map domain types to widget types at render time (e.g., `InsightSeverity` → `HealthStatus`)

For comprehensive UI architecture guidelines including module organization, action patterns, and anti-patterns to avoid, see [docs/UI_GUIDELINES.md](docs/UI_GUIDELINES.md).

### String Handling

Prefer isolated helper functions in `ui/helpers.rs` for string operations, especially truncation and display formatting. Rust strings are UTF-8, and byte-based slicing (`&s[..n]`) will panic if `n` falls inside a multi-byte character. Use:

- `truncate_left(s, max_chars)` → `...visible_end` (for paths)
- `truncate_right(s, max_chars)` → `visible_start...` (for tags/labels)

Never use `s.len()` for display width or `&s[..n]` for truncation on user-facing strings.

### DecisionWitness - SEALED ACCESS PATTERN

`DecisionWitness` can ONLY be created via `Witch::with_operator_decision()`, which should ONLY be called from `ui/operator_decisions.rs`. This module is the **operator confirmation boundary**.

**If you need to queue mutations from UI code:**

1. Call a function from `ui/operator_decisions.rs`:
   - `operator_decisions::stage_decision()` - add decision to active transaction
   - `operator_decisions::commit_transaction()` - commit all staged decisions
   - `operator_decisions::discard_transaction()` - discard all staged decisions

2. These functions are called ONLY from Enter keypress handlers in confirmation modals.

**DO NOT:**
- Call `with_operator_decision()` directly from action handlers or other UI code
- Add new functions to `operator_decisions.rs` without explicit human approval
- Try to work around this pattern to "simplify" mutation queueing

**Why this exists:** Previous versions had a public `confirm_decision()` function that could be called from anywhere, making it easy to accidentally bypass operator confirmation. The sealed module pattern ensures all decision authority flows through one small, auditable file.

**Exception:** Database creation in `first_time_setup.rs` doesn't need a witness - it's infrastructure setup, not a corpus mutation. Only mutations that alter indexed corpus data require DecisionWitness.

### Signal Design Principles

NOTE: slightly stale section. We now have a 'dirty inodes' table that we use for flagging inodes as dirty after a mutation => re-compute inode level signals next stage.

Most signals key off of inodes, or are otherwise tag-y/corpus-aggregate signals that usually benefit from reasoning about the full state of the corpus rather than the single-inode level.

**Signals must be small and individual.** Each signal should correspond to exactly one file, track, or piece of metadata - never aggregate/macro-level state.

**Good signals:**
- `UnindexedFile` for path X (one file)
- `LibraryLeftover` for library file Y (one file)
- `FingerprintDuplicate` for fingerprint Z (one group of tracks)
- `MissingTag` for tag T (one tag type across affected tracks)

**Bad signals (DO NOT CREATE):**
- `LibraryHealthSummary` (aggregate counts - compute at query time)
- `LibraryNotDeployed` (inverts the model - track "should be" somewhere)
- Any "summary" or "aggregate" signal that counts other signals
- Any signal that requires iterating ALL items to emit/update

**Why this matters:** If a computation runs per-directory (N directories) and each emits signals for ALL items (M items), you get N×M operations. With freshness checks this becomes N×M reads. Signals should be emitted by the computation that discovers the individual fact, not by aggregate passes.

**Aggregate information** (counts, summaries, "library health") should be:
1. Computed via SQL queries at UI time
2. Cached in `UiCache` with appropriate refresh intervals
3. NEVER stored as signals in the database

**Computations** should:
1. Emit signals for individual items they discover
2. Spawn follow-up computations for items needing further analysis
3. NOT iterate over "all items in the system" to emit global signals

### Documentation Requirements

When modifying computations (`meta::computations`), mutations (`meta::mutations`), or signals (`meta::signals`), you MUST update the corresponding reference documentation:

| Changed | Update |
|---------|--------|
| Computation logic, spawn behavior, signal emission | `docs/COMPUTATION_REFERENCE.md` |
| Mutation behavior, spawned computations, signal effects | `docs/MUTATION_REFERENCE.md` |
| Signal types, emitters, clearers | `docs/SIGNAL_REFERENCE.md` |

**Requirements:**
1. Update the relevant reference doc before or alongside code changes
2. Ensure "Emitted By" / "Cleared By" / "Spawns" columns remain accurate
3. Add new entries before implementing new signal types or computations

These documents are the source of truth for understanding system behavior. The source code comments reference them, and discrepancies should be treated as bugs.

### Dead Code Policy

**Never use `#[allow(dead_code)]`.** Dead code accumulates and rots. Instead:

1. **If code is vestigial** (was used, no longer is): Remove it entirely. Stub callers with `todo!("reconnect when X is implemented")`.

2. **If code is forward-looking** (building systems to connect later): New code should largely always be connected at this point. Un-integrated new code should be reported back as explicitly needing to be integrated and have a todo!("call this") to be removed once integrated.

3. **If removing would be expensive**: Prefer wholesale removal over surgical extraction. Rip out entire subsystems and leave `todo!()` stubs at the call sites.

4. **For unused imports**: Remove them. Don't annotate with `#[allow(unused_imports)]` "for future use" - imports are trivial to re-add.

**Rationale**: `#[allow(dead_code)]` silences the compiler's useful signal that code is disconnected. Over time, allowed dead code diverges from the live codebase (API changes, pattern evolution) making eventual reconnection harder than rewriting. The compiler warning is a feature, not noise. Explicit `todo!()` stubs are preferable because they're searchable, intentional, and will panic loudly if accidentally reached.

**Exceptions**: Test utilities (`#[cfg(test)]` modules) may have helpers not used by all tests.
