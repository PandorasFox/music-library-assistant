# MLA - Claude Development Notes

Check the docs/ folder for the Architecture overview and UI guidelines/UX consistency notes.

Do not spawn threads, ever. Only the task daemon should spawn new threads for background work. That is *its* purpose: consistent and clean way for doing background work, with controlled gates for their side effects.

Do not try to "refresh" signals. MLA is designed around precisely recomputing relevant signals in real-time. You keep adding unused (!) refresh hooks that then only get misused, because they're not things we need or want architecturally. They are expensive.

### Database Access Patterns

MLA enforces strict separation between read-only UI queries and write mutations:

**Read-Only Access (UI Code):**
- All UI code uses `daemon.read_only_db()` for database queries
- The daemon caches a single read-only connection (`PRAGMA query_only = ON`)
- This prevents accidental writes from UI code paths

**Write Access (Worker Threads Only):**
- Only daemon worker threads create write connections via `Database::open()`
- Mutations require `MutationExecutionWitness` tokens (zero-sized proof types)
- Migrations require `DecisionWitness` via explicit operator confirmation
- Write connections are created inside `execute_mutation()` and `execute_migration()`

**Exceptions:**
- First-time setup (`startup/first_time_setup.rs`) creates new database with write access
- Pre-App startup migrations (`check_and_run_migrations`) need write access before daemon exists

**Anti-patterns:**
- Never call `Database::open()` directly in UI code
- Never pass write connections from UI to mutation contexts
- Never create Database connections in rendering/display code

This pattern ensures all mutations are properly witnessed and attributable to operator decisions, enforcing the "operator-driven" principle from PHILOSOPHY.md.

### UI Caching Strategies

**Never query the database directly from render code.** DB queries during render cause multi-second frame times when workers are active, and create SQLite contention with write operations.

Two caching strategies exist for different use cases:

**1. UiCache (`ui/cache.rs`) - For "lively" data that updates during operation**

Use for data that changes while the user watches: corpus summary, daemon status, health stats.

```rust
// In event loop (ui/mod.rs run_app):
if let Some(ref mut daemon) = app.task_daemon {
    app.ui_cache.refresh(daemon);  // Refreshes stale cached values
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
// Query once during modal/view initialization:
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
- `daemon.read_only_db().get_*()` in render functions
- Passing `&Database` to render/display code
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

### Operator Decisions

**Core invariant: MLA never makes Decisions or Mutations autonomously.** All corpus Mutations must be attributable to explicit operator Decisions. See `docs/PHILOSOPHY.md` for full rationale.

DecisionWitness and ExecutionWitnesses are our methods of guaranteeing this.

### Signal Design Principles

**Signals must be small and individual.** Each signal should correspond to exactly one file, track, or piece of metadata - never aggregate/macro-level state.

**Good signals:**
- `UnindexedFile` for path X (one file)
- `LibraryOrphan` for library file Y (one file)
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

### Dead Code Policy

**Never use `#[allow(dead_code)]`.** Dead code accumulates and rots. Instead:

1. **If code is vestigial** (was used, no longer is): Remove it entirely. Stub callers with `todo!("reconnect when X is implemented")`.

2. **If code is forward-looking** (building systems to connect later): New code should largely always be connected at this point. Un-integrated new code should be reported back as explicitly needing to be integrated and have a todo!("call this") to be removed once integrated.

3. **If removing would be expensive**: Prefer wholesale removal over surgical extraction. Rip out entire subsystems and leave `todo!()` stubs at the call sites.

4. **For unused imports**: Remove them. Don't annotate with `#[allow(unused_imports)]` "for future use" - imports are trivial to re-add.

**Rationale**: `#[allow(dead_code)]` silences the compiler's useful signal that code is disconnected. Over time, allowed dead code diverges from the live codebase (API changes, pattern evolution) making eventual reconnection harder than rewriting. The compiler warning is a feature, not noise. Explicit `todo!()` stubs are preferable because they're searchable, intentional, and will panic loudly if accidentally reached.

**Exceptions**: Test utilities (`#[cfg(test)]` modules) may have helpers not used by all tests.
