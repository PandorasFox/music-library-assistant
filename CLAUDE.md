# MLA - Claude Development Notes

## Project Status

- **Status**: Work In Progress
- **Stability**: Experimental
- **Backwards Compatibility**: None guaranteed
- **Audience**: Personal use, eventual private testing with friends

This is prototyping software. Do not waste effort on backwards compatibility concerns.

## Design Philosophy

See [docs/PHILOSOPHY.md](docs/PHILOSOPHY.md) for the full conceptual foundation. Key principles:

1. **Librarian Cycle**: Insight → Intake → Organization → Deployment → Repair (repeat)
2. **Algebraic Changes**: Mutations are composable, reversible functions - preview before commit
3. **Atomic Operations**: Well-tested primitives applied in bulk without fear
4. **Dialogue Interface**: Structured conversation, one decision at a time

When implementing new features, ask:
- Which phase of the librarian cycle does this belong to?
- What change records does this operation produce?
- Can this be tested atomically and applied in bulk?
- How would the operator confirm this action in dialogue?

## Architecture Overview

### Core Design Principles

1. **Read-only by default**: Analysis operations never modify corpus
2. **Explicit mutations**: All corpus changes require explicit confirmation
3. **Hard link deployment**: Libraries reference corpus via hard links (no duplication)
4. **Incremental scanning**: Track files by inode+mtime to avoid unnecessary reads
5. **Algebraic changes**: Track mutations as composable, reversible functions

### Module Responsibilities

| Module | Purpose |
|--------|---------|
| `main.rs` | Entry point, config loading, TUI launch |
| `config.rs` | KDL config parsing, path utilities, logging |
| `db.rs` | Central data layer - all SQLite operations, types |
| `scanner.rs` | Directory walking, metadata extraction coordination |
| `metadata.rs` | Audio file metadata and fingerprint extraction |
| `deduplication.rs` | Fingerprint-based duplicate detection and resolution |
| `deploy.rs` | Library deployment via hard links |
| `reports.rs` | Analysis report generation |
| `changes.rs` | Algebraic change tracking and execution |
| `ui/mod.rs` | TUI entry point and mode dispatch |
| `ui/app.rs` | Application state, eye animation, operation tracking |
| `ui/main_menu.rs` | Multi-pane category/command navigation |
| `ui/menu.rs` | Legacy TUI state machine (being refactored) |
| `ui/picker.rs` | Reusable list picker component |

### Key Patterns

- **UiMode enum**: Top-level mode dispatch (MainMenu, TagEditor, Dialogue)
- **MenuState enum**: State machine for legacy TUI navigation (being phased out)
- **ScanProgress/ScanMessage**: Async progress updates via mpsc channels
- **Track struct**: Universal audio file representation
- **PendingChange**: Algebraic mutation representation
- **ConflictSet**: Duplicate grouping for resolution

### Data Locations

- **Config**: `$XDG_CONFIG_HOME/mla/config.kdl` (or `~/.config/mla/`)
- **Database**: `$XDG_DATA_HOME/mla/mla.db` (or `~/.local/share/mla/`)
- **Reports**: `$XDG_DATA_HOME/mla/reports/`
- **Logs**: `/tmp/mla.log`

## Librarian Workflow Implementation

### Menu Organization

The main menu reflects the librarian cycle:

| Category | Cycle Phase | Commands |
|----------|-------------|----------|
| Build Indices | Insight (prep) | Scan Corpus, Scan Legacy |
| Insight & Health | Insight | Reports (quality, duplicates, deployment) |
| Corpus-mutating Ops | Organization | Deduplication, Tag Editor |
| Deployment | Deployment | Preview, Deploy, View Changes |
| Intake | Intake | (Coming Soon) |

### Lost-Files Workspace

The `lost-files` directory serves as a staging area for:
- Duplicate files identified for removal
- Damaged files awaiting repair
- Files to be regenerated from external sources

Files can be moved here, repaired/regenerated, then re-entered via intake.

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

## Future Considerations (Not Yet Implemented)

### Database Snapshotting
- Export metadata as JSON for backup
- Import from snapshot to rebuild database
- Enable rollback after failed experiments

### External Metadata Integration
- MusicBrainz, Discogs, Beatport lookups
- Confidence levels per metadata source
- High-confidence sources can "freeze" corpus tags

### Corpus Directory Mapping
- Map corpus directories to external source identifiers
- e.g., `web/releases/bandcamp/artist` -> MusicBrainz artist ID
- Enable automated metadata lookup by directory structure
- Record label + catalog number as preferred canonical scheme

### Deployment Path Structuring
- Nuanced path structuring for vocalist/remix placement
- Configurable via "Opinions" in config.kdl
- Drive dialogue to establish operator's consistent preferences

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
