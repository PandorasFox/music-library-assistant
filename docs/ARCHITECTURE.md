# MLA Architecture

This document describes the high-level architecture of MLA, including module organization, data flows, and key abstractions.

---

## Module Organization

MLA's source code is organized into three primary domains plus configuration:

```
src/
├── main.rs          # Entry point
├── config.rs        # Configuration loading and path utilities
│
├── corpus/          # Corpus domain - indexing, health, analysis
│   ├── db/          # Database layer
│   │   ├── types.rs     # Track, HealthIssue, DeploymentStats
│   │   ├── queries/     # All database operations
│   │   ├── changes.rs   # PendingChange, ChangeType, ChangeStatus
│   │   └── decisions.rs # Decision flow types
│   ├── metadata.rs      # Audio file metadata and fingerprint extraction
│   ├── health/          # Health detection and signals
│   ├── deduplication/   # Duplicate detection algorithms
│   ├── fingerprint/     # Fingerprint quality analysis
│   └── reports/         # Report generation
│
├── ops/             # Operations domain - executing changes
│   ├── scanner.rs   # Directory walking and indexing
│   ├── progress.rs  # Scan progress tracking types
│   ├── changes.rs   # Change execution engine
│   ├── deploy.rs    # Library deployment via hard links
│   ├── dedup.rs     # Deduplication operations
│   ├── operation.rs # Background operation management
│   └── reports.rs   # Report execution
│
└── ui/              # User interface domain
    ├── mod.rs       # App state machine and mode dispatch
    ├── render.rs    # TUI rendering
    ├── app.rs       # Application state (eye animation, operations)
    ├── main_menu.rs # Category/command navigation
    ├── dialogue.rs  # Conversational decision flows
    ├── helpers.rs   # Shared rendering utilities
    ├── picker.rs    # Reusable list picker component
    ├── tag_editor/  # Multi-track metadata editing
    ├── corpus_browser/ # Directory tree browsing
    ├── dir_browser/    # Generic directory browser
    ├── canon_flow/     # Artist canonicalization
    ├── album_flow/     # Album canonicalization
    ├── album_artist_flow/ # Album artist resolution (3 phases)
    ├── dedup_flow/     # Fingerprint duplicate resolution
    ├── deploy_flow/    # Deployment preview
    └── drop_flow.rs    # Drop missing from index
```

### Domain Responsibilities

| Domain | Purpose |
|--------|---------|
| **corpus** | Indexing, analysis, and health tracking. Read-heavy operations that build understanding of the corpus state. |
| **ops** | Executing mutations. Write operations that modify files, database, or deployments. |
| **ui** | User interaction. Rendering, input handling, and workflow orchestration. |
| **config** | Application configuration and path utilities. |

---

## Corpus Indexing Process

The corpus indexing process maintains a database representation of audio files.

### Scan Flow

```
┌─────────────┐     ┌──────────────┐     ┌─────────────┐
│   Scanner   │────▶│   Metadata   │────▶│  Database   │
│  (walk dir) │     │  (extract)   │     │  (persist)  │
└─────────────┘     └──────────────┘     └─────────────┘
       │                   │
       ▼                   ▼
  scan_state          tracks table
  (inode+mtime)       (full metadata)
```

1. **Directory Walking**: Scanner walks corpus root, collecting audio files
2. **Incremental Detection**: Files checked against `scan_state` table (inode + mtime)
3. **Metadata Extraction**: Changed files have metadata extracted (tags, duration, bitrate)
4. **Fingerprint Generation**: Chromaprint fingerprints computed for duplicate detection
5. **Database Persistence**: Track records inserted/updated in `tracks` table

### Scan State Optimization

The `scan_state` table enables fast incremental scans:

- Stores `(source, path, inode, mtime)` for each scanned file
- On rescan, files with matching inode+mtime are skipped
- Files with changed mtime are re-scanned for updated metadata
- Files with new inodes are fully processed

---

## Health Signals

Health signals are conditions detected in the corpus that may require attention or resolution.

### Signal Types

| Signal | Severity | Description |
|--------|----------|-------------|
| `FingerprintDuplicate` | Auto/Manual | Same audio fingerprint across multiple files |
| `MetadataDuplicate` | Manual | Same artist/album/title, different fingerprints |
| `TagCanonical` | Informational | Tag values that could be unified (spelling variants) |
| `MissingTag` | Informational | Required tags missing (e.g., album_artist) |
| `QualityVariant` | Auto | Same content at different quality levels |
| `DeployConflict` | Manual | Multiple files would deploy to same path |
| `OutOfBandTagChange` | Manual | On-disk tags differ from indexed values |
| `MissingFromIndex` | Informational | Files on disk not yet indexed |

### Health Detection Points

Health issues are detected at several points:

1. **Scan Time**: During indexing, detect duplicates and tag issues
2. **Heartbeat**: At startup, quick validation of corpus vs index
3. **Resolution Flows**: User-driven canonicalization discovers tag variants
4. **Mutation Time**: After tag edits, detect out-of-band changes

### Health State Invariants

The health system maintains these invariants:

1. **Unique Issue Keys**: Each health issue has a unique `(type, key)` pair
2. **Track Membership**: Issues link to affected tracks via `health_issue_tracks`
3. **Resolution Tracking**: Resolved issues retain resolution metadata
4. **Session Association**: Resolutions link to the session that resolved them

---

## Operational Resolution Flows

Resolution flows are multi-step user interactions that resolve health issues or perform bulk operations.

### Flow Pattern

All resolution flows follow a common pattern:

```
┌──────────────┐     ┌──────────────┐     ┌──────────────┐
│   Cluster    │────▶│    Review    │────▶│    Commit    │
│    View      │     │    State     │     │   Handler    │
└──────────────┘     └──────────────┘     └──────────────┘
       │                    │                    │
   Selection           Decisions            Execution
   + Decision          + Preview            + Persist
```

1. **Cluster View**: Present grouped items for selection/decision
2. **Review State**: Collect decisions, show summary before commit
3. **Commit Handler**: Execute changes, update database, create pending changes

### Available Flows

| Flow | Purpose | Health Signal Resolved |
|------|---------|----------------------|
| `canon_flow` | Artist name canonicalization | TagCanonical |
| `album_flow` | Album name canonicalization | TagCanonical |
| `album_artist_flow` | Album artist resolution (3 phases) | MissingTag, TagCanonical |
| `dedup_flow` | Fingerprint duplicate resolution | FingerprintDuplicate |
| `deploy_flow` | Library deployment preview | DeployConflict |

### Algebraic Change Tracking

All corpus mutations are tracked as composable, reversible operations:

```rust
PendingChange {
    change_type: TagEdit | Move | Delete | Deploy | Undeploy,
    source_path: String,
    target_path: Option<String>,
    metadata_changes: Option<JSON>,
    status: Pending | Staged | Committed | Reverted,
}
```

Changes accumulate in `pending_changes`, can be previewed, and committed atomically.

---

## Database Architecture

The database is SQLite-based with tables organized by function.

### Core Tables

| Table | Purpose |
|-------|---------|
| `tracks` | Audio file metadata and fingerprints |
| `scan_state` | Incremental scan tracking (inode + mtime) |
| `pending_changes` | Accumulated mutations awaiting commit |
| `change_sessions` | Groups of related changes |

### Health Tables

| Table | Purpose |
|-------|---------|
| `health_issues` | Detected health problems |
| `health_issue_tracks` | Track membership in issues |
| `known_variants` | Accepted duplicates (re-releases, etc.) |
| `tag_canonicalizations` | Canonical tag value mappings |
| `tag_mismatches` | DB vs on-disk tag differences |

### Deployment Tables

| Table | Purpose |
|-------|---------|
| `deployments` | Track deployments to libraries |
| `deployment_orphans` | Library files not linked to corpus |

See [DATABASE.md](DATABASE.md) for complete schema documentation.

---

## UI State Machine

The UI uses a mode-based state machine for navigation:

```rust
enum UiMode {
    MainMenu,           // Category/command navigation
    TagEditor,          // Multi-track metadata editing
    Dialogue,           // Conversational decision flow
    DialogueSummary,    // Summary before commit
    // Resolution flows
    CanonClusterView, CanonSessionReview,
    AlbumClusterView, AlbumReview,
    // ... etc
}
```

### Mode Transitions

- Menu commands trigger transitions via `CommandAction`
- Flows advance through phases (Cluster → Review → Commit)
- Escape generally returns to previous mode or main menu
- Background operations update state via channels

---

## Background Operations

Long-running operations execute in background threads:

```rust
OperationType {
    Scanning { source, path },
    Fingerprinting { source },
    ExecutingChanges { session_id, count },
}
```

### Operation Lifecycle

1. **Spawn**: Create thread with progress channel
2. **Progress**: Send `OperationProgress` updates via channel
3. **Completion**: Send `OperationResult` with success/failure
4. **UI Update**: Main loop polls channels, updates display

---

## Configuration

Configuration is loaded from `$XDG_CONFIG_HOME/mla/config.kdl`:

```kdl
corpus-root "/path/to/corpus"

library "main" {
    path "/path/to/library"
}

legacy-library {
    path "/path/to/legacy"
}
```

### Path Conventions

| Path | Purpose |
|------|---------|
| `$XDG_CONFIG_HOME/mla/` | Configuration files |
| `$XDG_DATA_HOME/mla/` | Database, reports |
| `/tmp/mla.log` | Debug logging |

---

## Extension Points

### Adding a New Health Signal

1. Add variant to `HealthIssueType` enum in `db/types.rs`
2. Add string mapping in `as_str()` and `from_str()`
3. Create detection function in `corpus/health/detection.rs`
4. Call detection at appropriate point (scan, heartbeat, mutation)

### Adding a New Resolution Flow

1. Create flow module in `ui/` (e.g., `ui/my_flow/`)
2. Define state types, actions, and rendering
3. Add `UiMode` variant for each flow phase
4. Add state field to `App` struct
5. Wire up mode dispatch in `handle_key()` and `render()`
6. Add menu command to trigger flow

### Adding a New Report

1. Add report function in `corpus/reports/`
2. Add `ReportType` variant in menu
3. Wire up in `generate_report()` handler
