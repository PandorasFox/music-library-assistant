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
| `db/decisions.rs` | Decision flow types for dialogue system |
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
| `ui/main_menu.rs` | Multi-pane category/command navigation |
| `ui/dialogue.rs` | Conversational decision flow UI |
| `ui/tag_editor/` | Multi-track metadata editing module |
| `ui/picker.rs` | Reusable list picker component |
| `ui/helpers.rs` | Shared rendering utilities and formatters |

### Key Patterns

- **UiMode enum**: Top-level mode dispatch (MainMenu, TagEditor, Dialogue)
- **MenuState enum**: State machine for legacy TUI navigation (being phased out)
- **ScanProgress/ScanMessage**: Async progress updates via mpsc channels
- **Track struct**: Universal audio file representation
- **PendingChange**: Algebraic mutation representation
- **ConflictSet**: Duplicate grouping for resolution

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

### Algebraic Change Tracking

All corpus-mutating operations are tracked as composable functions:

```rust
PendingChange {
    change_type: Move | Delete | TagEdit | Deploy | Undeploy
    source_path: String
    target_path: Option<String>
    metadata_changes: Option<JSON>
    status: Pending | Staged | Committed | Reverted
}
```

Changes accumulate in `pending_changes` table, can be previewed, staged to a preview library, then committed or discarded.

## Database Schema

Key tables:
- `tracks`: Audio file metadata, fingerprints, ISRC codes
- `scan_state`: Incremental scan tracking (inode + mtime)
- `pending_changes`: Accumulated mutations awaiting commit
- `change_sessions`: Groups of related changes

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
