# MLA - Claude Development Notes

Check the docs/ folder for the Architecture overview and UI guidelines/UX consistency notes.

Do not spawn threads, ever. Only the task daemon should spawn new threads for background work. That is *its* purpose: consistent and clean way for doing background work, with controlled gates for their side effects.

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

### Dead Code Policy

**Never use `#[allow(dead_code)]`.** Dead code accumulates and rots. Instead:

1. **If code is vestigial** (was used, no longer is): Remove it entirely. Stub callers with `todo!("reconnect when X is implemented")`.

2. **If code is forward-looking** (building systems to connect later): New code should largely always be connected at this point. Un-integrated new code should be reported back as explicitly needing to be integrated and have a todo!("call this") to be removed once integrated.

3. **If removing would be expensive**: Prefer wholesale removal over surgical extraction. Rip out entire subsystems and leave `todo!()` stubs at the call sites.

4. **For unused imports**: Remove them. Don't annotate with `#[allow(unused_imports)]` "for future use" - imports are trivial to re-add.

**Rationale**: `#[allow(dead_code)]` silences the compiler's useful signal that code is disconnected. Over time, allowed dead code diverges from the live codebase (API changes, pattern evolution) making eventual reconnection harder than rewriting. The compiler warning is a feature, not noise. Explicit `todo!()` stubs are preferable because they're searchable, intentional, and will panic loudly if accidentally reached.

**Exceptions**: Test utilities (`#[cfg(test)]` modules) may have helpers not used by all tests.
