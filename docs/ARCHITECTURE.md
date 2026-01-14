# MLA System Architecture

This document describes the major subsystems of MLA and how they interact. For the conceptual foundations and design principles, see [PHILOSOPHY.md](PHILOSOPHY.md).

---

## System Overview

```
┌────────────────────────────────────────────────────────────────┐
│                              FILESYSTEM                        │
│  ┌───────────────┐    ┌───────────────┐    ┌───────────────┐   │
│  │    Corpus     │    │   Libraries   │    │     Inbox     │   │
│  │   (source)    │    │  (hard-links) │    │  (incoming)   │   │
│  └───────┬───────┘    └───────────────┘    └───────────────┘   │
└──────────┼─────────────────────────────────────────────────────┘
           │                                        
           ▼                                        
    ┌─────────────┐                                 
    │  Heartbeat  │                                 
    │   (poll)    │                                 
    └──────┬──────┘                                 
           │ compare                                
           └────────────────────┐                    
┌─────────────────────────────────────────────────────────────────┐
│                              DATABASE                           │
│  ┌───────────────┐    ┌───────────────┐    ┌───────────────┐    │
│  │     Index     │    │   Insights    │    │   Audit Log   │    │
│  │   (tracks,    │    │   (signals)   │    │  (decisions,  │    │
│  │  scan_state)  │    │               │    │   mutations)  │    │
│  └───────────────┘    └───────┬───────┘    └───────▲───────┘    │
└───────────────────────────────┼────────────────────┼────────────┘
                                │ surface            │ log
                                ▼                    │
                    ┌───────────────────────┐        │
                    │         USER          │        │
                    │    (main menu UI)     │        │
                    └───────────┬───────────┘        │
                                │ select signal      │
                                ▼                    │
                    ┌───────────────────────┐        │
                    │   Operation Flow      │        │
                    │   (by signal type)    │        │
                    └───────────┬───────────┘        │
                                │                    │
                                ▼                    │
                    ┌────────────────────────┐       │
                    │      Decisions         │       │
                    │ (accumulate; generates │       │
                    │     mutations)         │       │
                    └───────────┬────────────┘       │
                                │                    │
                                ▼                    │
                    ┌───────────────────────┐        │
                    │       Review          │        │
                    │  (preview mutations)  │        │
                    └─────┬─────────┬───────┘        │
                          │         │                │
                 discard  │         │ confirm        │
                          ▼         ▼                │
                        ┌───┐   ┌───────────────┐    │
                        │ X │   │  Background   │────┘
                        └───┘   │   Executor    │
                                └───────┬───────┘
                                        │
           ┌────────────────────────────┼────────────────────────────┐
           │                            │                            │
           ▼                            ▼                            ▼
    ┌─────────────┐              ┌─────────────┐              ┌─────────────┐
    │   Corpus    │              │    Index    │              │  Insights   │
    │  (files)    │              │ (database)  │              │  (signals)  │
    └─────────────┘              └─────────────┘              └─────────────┘
```

---

## The Core Loop

After initial corpus indexing, MLA operates in a continuous cycle:

```
┌─────────────────────────────────────────────────────────────────────┐
│                                                                     │
│   Heartbeat ──► Signals ──► Insights ──► Operations ──► Mutations   │
│                   ▲                                          │      │
│                   │                                          │      │
│                   └──────────────────────────────────────────┘      │
│      (mutations update corpus state and therefore signals)          │
│                                                                     │
└─────────────────────────────────────────────────────────────────────┘
```

1. **Heartbeat** polls the corpus directories, comparing against the index
2. **Signals** are aggregated and surfaced as insights
3. **Insights** surface signals in bulk fashion to the operator
4. **Operations** are flows the user launches, with intent to resolve specific signals
5. **Decisions** are presented to the operator as simple but sweeping choices in flows, and can generate many associated mutations
6. **Review** presents the batch for confirmation or discard
7. **Background Executor** applies confirmed mutations to corpus, index, and insights, and provides a single, well-reasoned entrypoint to applying small, easy-to-reason-about changes in bulk scale.
8. **Audit Log** records decisions and mutations for potential future reversion

---

## Subsystem Details

### 1. Heartbeat (Corpus Survey)

The heartbeat is MLA's background pulse. It polls the corpus directories and compares against the index.

**Trigger**: At startup, and periodically during idle (triggered probabilistically via d20 roll on eye blink animation).

**Responsibilities**:
- Walk corpus directory trees
- Compare file state against index (inode, mtime, presence)
- Detect new files, missing files, relocations, out-of-band changes
- updates health signals based on changes in filesystem state, if any, as signals are computed as a function of (state of filesystem, state of indices) or (state of indices) largely

**Key Constraint**: The heartbeat *detects* but does not *resolve*. It generates signals.

**Implementation**: `src/corpus/health/heartbeat.rs`

---

### 2. Corpus Index

The central database of all indexed audio files.

**Tables**:
| Table | Purpose |
|-------|---------|
| `tracks` | Audio file metadata, fingerprints, paths |
| `scan_state` | Inode/mtime tracking for incremental scans |
| `deployments` | Tracks → library deployment state |

**Key Principle**: The index is the source of truth for what MLA knows about the corpus. Files on disk may diverge (and that divergence becomes a signal as soon as it is detected).

**Implementation**: `src/corpus/db/`

---

### 3. Insights (Health Signals)

Signals are persistent facts about corpus health, stored in the database and surfaced to the user in aggregate as insights. Insights are basically just summaries about different signals, but we do want to thoughtfully combine some different signals at our disposal, such as which fingerprint dupe signals coexist alongside QualityVariant signals - those should be easy to surface in a flow and resolve!

**Signal Types**:

| Signal | Description | Severity |
|--------|-------------|----------|
| `FingerprintDuplicate` | Same audio fingerprint across files | Medium |
| `TagCanonical` | Tag variants needing canonicalization | Low |
| `MissingTag` | Required tags missing (e.g., album_artist) | Low |
| `QualityVariant` | Same content at different qualities, concordent with fingerprintduplicate usually | Low |
| `DeployConflict` | Multiple files would deploy to same path | Medium |
| `OutOfBandTagChange` | Disk tags differ from indexed tags | Medium |
| `MissingFromIndex` | Files on disk not in index | Medium |
| `FileRelocated` | File moved (same inode, new path) | Low |
| `DuplicateInode` | Multiple index entries share one inode | High |
| `MissingFromDisk` | Indexed file no longer exists | High |
| `OutOfBandFileChange` | File replaced externally | High |

**Lifecycle**:
1. Pushed by heartbeat or mutation side-effects
2. Surfaced to user via insights menu
3. Cleared when resolved via operation flows

**Key Principle**: Signals are computed facts, not actions. They describe *what is*, not *what to do*.

**Implementation**: `src/corpus/health/detection.rs`, `src/corpus/db/types.rs`

---

### 4. Operation Flows (Signal Resolution)

Operation flows are the primary way users resolve signals based on Insights. 

**The key design principle**:

> Present as FEW choices as possible, with as LARGE an impact as possible.

**Examples**:
- Fingerprint quality dedup: "Which of these files are identical, tagged identically, and are outranked by a better bitrate dupe?" -> potentially affects thousands of tracks at once
- Artist canonicalization: "Which spelling of 'DragonForce' should be canonical?" → affects dozens of tracks
- Duplicate resolution: "Which of these directories outranks the others?" → resolves hundreds of dupes
- Deploy conflicts: "Which track should occupy this library slot?" → one or two decisions per conflict, lots of manual tag editing, last flow to do generally

**Flow Structure**:
```
Insight into combination of signals
    │
    ▼
Operation Flow (by Insight)
    │
    ├─► Decision 1 ──► accumulates mutations
    ├─► Decision 2 ──► accumulates mutations
    └─► Decision N ──► accumulates mutations
            │
            ▼
        Review (preview all accumulated mutations)
            │
            ├─► Discard ──► [X] terminate
            │
            └─► Confirm ──► Background Executor
                                │
                                ├─► modify corpus files
                                ├─► modify inbox files
                                ├─► update index
                                ├─► update/clear signals
                                └─► write audit log
```

**Key Principle**: Operations produce mutations. They never directly modify files.

**Implementation**: `src/flows/`, `src/ui/*_flow/`

---

### 5. Algebraic Mutations

**ALL CORPUS CHANGES MUST GO THROUGH THE MUTATIONS SYSTEM.**

Mutations are algebraic expressions of change. They are:
- **Composable**: Mutations can spawn child mutations (DAG structure)
- **Accumulable**: Many mutations queue up during a flow before execution
- **Previewable**: Operator sees summary before committing
- **Attributable**: Tied to high-level Decisions for audit trail

**Mutation Categories**:

| Category | Mutations |
|----------|-----------|
| Tag editing | `TagEditDb`, `TagEditFile`, `TagEdit` (composite) |
| File operations | `MoveFile`, `DeleteFile`, `StashFile` |
| Index operations | `DropFromIndex`, `UpdateTrack`, `UpdateTrackPath` |
| Deployment | `Deploy`, `Undeploy`, `RefreshDeployment` |
| Scan state | `UpdateScanStatePath`, `RemoveScanState` |

**Composition Example** (DAG):
```
TagEdit(file, field, value)
    ├─► TagEditDb(file, field, value)   // Update index
    └─► TagEditFile(file, field, value) // Flush to disk
```

This composition allows granular reuse. For example, resolving `OutOfBandTagChange` might use `TagEditDb` alone (to accept disk reality) or `TagEditFile` alone (to restore indexed values).

**Key Constraint**: Mutations execute via the background task system. They can always occur in large numbers and must not block the UI.

**Implementation**: `src/corpus/mutations/`

---

### 6. Audit Log (Future)

The audit log records decisions and their resulting mutations, enabling:
- Traceability: "Why was this tag changed?"
- Potential reversion: Undo decisions by reversing their mutations
- Export: Plain-text dump of decision history for archival

**Status**: Planned. Currently mutations execute but are not audited beyond the session.

---

### 7. Hard-Link Deployments

Deployment is a signal resolution flow that deploys corpus files to library directories via hard links.

This one is pretty simple, honestly.

**Deployment Path**: `{library_root}/{album_artist}/{album}/{track}` (configurable)

**Key Properties**:
- Hard links only (no duplication of data)
- Full tag collisions are NOT deployed (keeps library duplicate-free)
- Deployment conflicts are pre-computed signals
- Stale files (tags changed after deployment) are tracked
- Orphan files (shouldn't exist) are stashed during next deploy

**Implementation**: `src/flows/deploy.rs`, `src/corpus/health/library.rs`

---

### 8. Intake Flow (Planned)

The intake flow processes incoming files from a configured inbox directory.

**Design**: The inbox is treated as a special directory processed through the usual file-moving mechanisms (like the stash). Files go through:
- Unpacking (if archives)
- Normalization against existing index
- Deduplication checks
- Atomic addition to corpus

**Relationship to Heartbeat**: Once intake is operational, heartbeat becomes primarily for detecting missing files, out-of-band changes, and relocations within the corpus itself.

**Status**: Planned, not yet implemented.

---

### 9. Corpus Browser

A standalone interface for browsing the corpus and editing tags.

**Features**:
- Directory tree navigation with track counts
- Metadata preview (bitrate, duration, sample rate, tags)
- Multi-track tag editor

**Integration with Mutations**: Tag edits from the browser generate mutations that flow through the standard accumulate → review → confirm pipeline.

**Key Principle**: The browser is a tool, not a flow. It provides direct access without the guided decision structure of operations.

**Implementation**: `src/ui/corpus_browser/`, `src/ui/tag_editor/`

---

### 10. Hello World (Setup Witch)

Onboarding flow for new users.

**Current**: Auto-detect empty database → start corpus scan

**Planned**:
- Detect missing config file
- Interactive wizard (witch) for initial configuration
- Initial library triage and recommended resolution actions

**Priority**: Very low. Other systems take precedence. Initial insights can be freely offered once Insights are fleshed out.

---

## Key Invariants

1. **Mutations are the only write path**: No system directly modifies corpus files. All changes go through mutations.

2. **Signals are cached facts**: They exist in the database, are pushed by heartbeat/mutations, and describe corpus state without prescribing action.

3. **Operations produce, never apply**: Operation flows generate mutations. The background executor applies them.

4. **Confirm before commit**: Accumulated mutations are always previewed before execution. No silent changes.

5. **Heartbeat detects, doesn't resolve**: The heartbeat pushes signals. Resolution is a separate user-driven process via operation flows.

6. **Background execution for bulk work**: Mutations can always occur in large numbers. The background task system handles execution without blocking the UI.

---

## Module Organization

MLA's source code is organized into three primary domains:

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
│   ├── mutations/       # Algebraic mutation types and executors
│   ├── deduplication/   # Duplicate detection algorithms
│   └── reports/         # Report generation
│
├── flows/           # Operation flows - UI-driving process logic
│   ├── scanner.rs   # Directory walking and indexing
│   ├── progress.rs  # Scan progress tracking types
│   ├── changes.rs   # Change execution engine
│   ├── deploy.rs    # Library deployment via hard links
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
    ├── widgets/     # Reusable UI components
    ├── tag_editor/  # Multi-track metadata editing
    ├── corpus_browser/ # Directory tree browsing
    ├── canon_flow/     # Artist canonicalization UI
    ├── album_flow/     # Album canonicalization UI
    ├── album_artist_flow/ # Album artist resolution UI
    ├── dedup_flow/     # Fingerprint duplicate resolution UI
    └── deploy_flow/    # Deployment preview UI
```

### Domain Responsibilities

| Domain | Purpose |
|--------|---------|
| **corpus** | Indexing, analysis, health tracking, and mutation definitions. The "model" layer. |
| **flows** | Process-driving logic for operations. Background execution, scanning, deployment logic. |
| **ui** | User interaction. Rendering, input handling, and flow state machines. |

---

## Extension Points

### Adding a New Health Signal

1. Add variant to `HealthIssueType` enum in `corpus/db/types.rs`
2. Add string mapping in `as_str()` and `from_str()`
3. Create detection function in `corpus/health/detection.rs`
4. Call detection at appropriate point (heartbeat or mutation side-effect)

### Adding a New Mutation

1. Add variant to `Mutation` enum in `corpus/mutations/types.rs`
2. Implement executor in `corpus/mutations/executor.rs`
3. If composite, define child mutations and DAG structure
4. Wire into `MutationDispatcher.execute_sync()`

### Adding a New Resolution Flow

1. Create flow module in `ui/` (e.g., `ui/my_flow/`)
2. Define state types, actions, and rendering
3. Add `UiMode` variant for each flow phase
4. Add state field to `App` struct
5. Wire up mode dispatch in `handle_key()` and `render()`
6. Add menu command to trigger flow from insights

### Adding a New Report

1. Add report function in `flows/reports.rs`
2. Add `ReportType` variant in menu
3. Wire up in `generate_report()` handler
