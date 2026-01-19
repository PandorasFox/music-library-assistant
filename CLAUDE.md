# MLA - Claude Development Notes

## Project Status

- **Status**: Work In Progress
- **Stability**: Experimental
- **Backwards Compatibility**: None guaranteed
- **Audience**: Personal use, eventual private testing with friends

This is prototyping software. Do not waste effort on backwards compatibility concerns.

## Design Philosophy

See [docs/PHILOSOPHY.md](docs/PHILOSOPHY.md) for the full conceptual foundation. The main takeaway is:

* Libraries need a cycle of health-maintaining processes as they grow.
* Librarians need a wide variety of basic tools to work on their curated corpus & how they present it
* All changes to the corpus must be modelled as algebraic. We want mutations to be composable and accumulatable
* MLA interactions are menu-driven, with bulk corpus mutation choices being presented as an almost-conversational series of dialog choices.
* MLA should defer *all decisions* to the operator. Some decisions can be explicitly configured as "Opinions" to automatically follow.

Additionally, MLA itself should be neatly compartmentalized and organized. "Everything has an appropriate home" applies to both items in our corpus, and MLA's codebase; we should make efforts to modularize and keep things tidy and tested.

When implementing new features, ask:
- Which phase of the librarian cycle does this belong to?
- How should this best be organized?
- Is there central overlap I could leverage to keep things integrated together cleanly?

---
content below this point has been authored by claude, and are claude's notes on current implementation.
---

## Architecture Overview

### Core Design Principles

1. **Read-only by default**: Analysis operations never modify corpus
2. **Algebraic changes**: Track mutations as composable, reversible functions
3. **Explicit mutations**: All corpus changes require explicit confirmation to commit
4. **Resource efficiency**: hard-link deployments only; track inode+mtimes for scan efficiency
5. **Keyboard ergonomics**: Arrow keys and Enter for navigation/selection only. No number hotkeys or letter shortcuts for quick-select in menus. Typing is reserved for data entry. Mouse clicks are acceptable if available.

### Module Responsibilities

| Module | Purpose |
|--------|---------|
| `main.rs` | Entry point, config loading, TUI launch |
| `config.rs` | KDL config parsing, path utilities, logging |
| `db/` | Central data layer module |
| `db/types.rs` | Track, ScanStateEntry, DeploymentStats |
| `db/changes.rs` | PendingChange, ChangeType, ChangeStatus, ChangeSession |
| `db/queries.rs` | All SQLite operations and Database methods |
| `scanner.rs` | Directory walking, metadata extraction coordination |
| `metadata.rs` | Audio file metadata and fingerprint extraction |
| `deduplication.rs` | Fingerprint-based duplicate detection and resolution |
| `deploy.rs` | Library deployment via hard links |
| `reports.rs` | Analysis report generation |
| `changes.rs` | Algebraic change tracking and execution |
| `ui/mod.rs` | TUI entry point and mode dispatch |
| `ui/render.rs` | TUI rendering functions |
| `ui/app.rs` | Application state, eye animation, operation tracking |
| `ui/tag_editor/` | Multi-track metadata editing module |
| `ui/tree_browser/` | Unified tree browser with variant modes (CorpusBrowser, DirectorySelector) |
| `ui/helpers.rs` | Shared rendering utilities and formatters |
| `ui/widgets/` | Reusable UI components (lists, layouts, status, modals) |
| `ui/insights_view/` | Real-time computed insights view (main entry point) |
| `corpus/health/insights/` | Insight computation (one-dim and multi-dim) |

### Key Patterns

- **UiMode enum**: Top-level mode dispatch (Insights, TagEditor, CorpusBrowser, etc.)
- **ScanProgress/ScanMessage**: Async progress updates via mpsc channels
- **Track struct**: Universal audio file representation
- **PendingDecision**: Operator-driven decision representation (in `flows/decisions.rs`)
- **ConflictSet**: Duplicate grouping for resolution

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
| `TwoPaneLayout`, `ThreePaneLayout` | Standard multi-pane layouts |
| `Modal`, `ConfirmationModal` | Popup dialogs |
| `ControlsHint` | Context-sensitive keyboard hints |

**Guidelines:**
1. Check `ui/widgets/` before creating new rendering code
2. If existing widgets don't fit, add to or extend them rather than creating ad-hoc rendering
3. Keep UI-agnostic logic (like severity levels) separate from widget styling
4. Map domain types to widget types at render time (e.g., `InsightSeverity` → `HealthStatus`)

### Three-Layer Health Architecture

```
Index (Database)
    ↓
Signals (Cached health_issues, health_resolutions)
    ↓
Insights (Real-time computed views, never stored)
```

- **Index**: Raw track data in `tracks` table
- **Signals**: Pre-computed facts stored in `health_issues` table (duplicates, tag conflicts, etc.)
- **Insights**: On-demand computed recommendations over signals, surfaced to the operator

Insights are categorized as:
- **One-Dimensional**: Immediate computation from single signal type + heartbeat
- **Multi-Dimensional**: Background computation correlating multiple signal types

### Active Flow Modules

| Module | Status | Notes |
|--------|--------|-------|
| `ui/tree_browser/` | Active | Unified tree browser: CorpusBrowser + DirectorySelector variants |
| `ui/deploy_flow/` | Active | Deployment preview and execution |
| `ui/insights_view/` | Active | Main entry point - lateral view ring |
| `ui/drop_flow.rs` | Active | Drop missing files from index |

Focus new work on the Insights system, which presents data to the user and provides entry points into resolution flows.

### String Handling

Prefer isolated helper functions in `ui/helpers.rs` for string operations, especially truncation and display formatting. Rust strings are UTF-8, and byte-based slicing (`&s[..n]`) will panic if `n` falls inside a multi-byte character. Use:

- `truncate_left(s, max_chars)` → `...visible_end` (for paths)
- `truncate_right(s, max_chars)` → `visible_start...` (for tags/labels)

Never use `s.len()` for display width or `&s[..n]` for truncation on user-facing strings.

### Data Locations

- **Config**: `$XDG_CONFIG_HOME/mla/config.kdl` (or `~/.config/mla/`)
- **Database**: `$XDG_DATA_HOME/mla/mla.db` (or `~/.local/share/mla/`)
- **Reports**: `$XDG_DATA_HOME/mla/reports/`
- **Logs**: `/tmp/mla.log`

### Operator Decisions

**Core invariant: MLA never makes Decisions or Mutations autonomously.** All corpus Mutations must be attributable to explicit operator Decisions. See `docs/PHILOSOPHY.md` for full rationale.

The flow is:
```
Operator Flow → PendingDecision[] (accumulated) → execute_decisions() → Mutations
```

Decision types (in `flows/decisions.rs`):

```rust
PendingDecision {
    decision_type: Move | Delete | DropIndex | TagEdit | Deploy | Undeploy | Redeploy
    source_path: String
    target_path: Option<String>
    metadata: Option<JSON>
}
```

Decisions are accumulated in-memory during UI flows, then executed in batch via `flows::changes::execute_decisions()`. There is no database persistence for pending decisions - they exist only within the lifetime of an operator flow.

## Database Schema

Key tables:
- `tracks`: Audio file metadata, fingerprints, ISRC codes
- `scan_state`: Incremental scan tracking (inode + mtime)
- `health_issues`: Detected corpus health problems
- `tag_canonicalization`: Artist/album/genre canonical mappings

Source names are lowercase: `corpus`, `legacy`, library names.

## Future Features

See [docs/FUTURE_FEATURES.md](docs/FUTURE_FEATURES.md) for planned features, improvements, and TODO items.

## Common Development Tasks

### Adding a New Menu Option

1. Determine which category in `build_menu_categories()` in `main_menu.rs`
2. Add `Command` with appropriate `CommandAction`
3. If `CommandAction::Background`, add handler in `mod.rs` `execute_menu_action()`
4. If `CommandAction::Transition`, ensure target UiMode is handled

### Adding a Database Table

1. Add schema in `initialize_schema()` in `db.rs`
2. Add struct for row type
3. Add CRUD methods
4. Add migration if table might not exist in older DBs

### Adding a New Report

1. Add function in `reports.rs`
2. Add variant to `ReportType` in `main_menu.rs`
3. Add case in `generate_report()` in `mod.rs`
4. Reports save to `~/.local/share/mla/reports/`

### Adding a Dialogue Flow

1. Add state struct in `ui/dialogue.rs` (when extracted)
2. Add transition from `CommandAction::Transition`
3. Implement `handle_key()` returning `DialogueAction`
4. Follow dialogue design principles from PHILOSOPHY.md

### Managing TODOs

All TODO/FIXME/HACK comments in code should be reflected in [docs/FUTURE_FEATURES.md](docs/FUTURE_FEATURES.md) under the "Technical Debt" section. This centralizes intent tracking for future cleanup sessions.

When adding a TODO in code:
1. Add the comment with file location context (e.g., `// TODO: description`)
2. Add corresponding entry to `docs/FUTURE_FEATURES.md` Technical Debt section with `file:line`

When resolving a TODO:
1. Remove the code comment
2. Remove the corresponding entry from `docs/FUTURE_FEATURES.md`

Periodically grep for `TODO|FIXME|HACK|XXX` and reconcile with the docs page.

### Dead Code Policy

**Never use `#[allow(dead_code)]`.** Dead code accumulates and rots. Instead:

1. **If code is vestigial** (was used, no longer is): Remove it entirely. Stub callers with `todo!("reconnect when X is implemented")`.

2. **If code is forward-looking** (building systems to connect later): New code should largely always be connected at this point. Un-integrated new code should be reported back as explicitly needing to be integrated and have a todo!("call this") to be removed once integrated.

3. **If removing would be expensive**: Prefer wholesale removal over surgical extraction. Rip out entire subsystems and leave `todo!()` stubs at the call sites.

4. **For unused imports**: Remove them. Don't annotate with `#[allow(unused_imports)]` "for future use" - imports are trivial to re-add.

**Rationale**: `#[allow(dead_code)]` silences the compiler's useful signal that code is disconnected. Over time, allowed dead code diverges from the live codebase (API changes, pattern evolution) making eventual reconnection harder than rewriting. The compiler warning is a feature, not noise. Explicit `todo!()` stubs are preferable because they're searchable, intentional, and will panic loudly if accidentally reached.

**Exceptions**: Test utilities (`#[cfg(test)]` modules) may have helpers not used by all tests.
