# MLA Future Features

This document catalogs planned features and improvements for MLA. Items are loosely categorized and should be considered a backlog rather than a roadmap.

---

## Database & Storage

### Database Snapshotting
- Export metadata as JSON for backup
- Import from snapshot to rebuild database
- Enable rollback after failed experiments

### Transaction Safety
- Add transaction log for file moves
- Enable rollback of entire sessions
- Consider SQLite savepoints for atomic operations

---

## External Metadata Integration

### MusicBrainz Integration
- Lookup tracks by fingerprint (AcoustID)
- Import release metadata (label, catalog number, dates)
- Map corpus directories to MusicBrainz release IDs

### Discogs Integration
- Lookup by release metadata
- Import vinyl/pressing details
- Cross-reference pricing data

### Beatport Integration
- Lookup by track metadata
- Import genre/subgenre tags
- BPM and key validation

### Confidence Levels
- Track metadata source provenance
- Assign confidence scores per field
- High-confidence sources can "freeze" corpus tags
- Conflict resolution when sources disagree

---

## Album Artist Health Restoration [IMPLEMENTED]

> Implemented in `src/ui/album_artist_flow/` with phase selector, canonicalization, collation, and population flows.

A unified meta-flow for resolving album_artist issues across the corpus. This is critical for proper deployment path computation since deployment paths use album_artist.

### Three Sub-Flows [COMPLETE]

1. **Album Artist Canonicalization** - `album_artist_flow/cluster_view.rs`
2. **Album Artist Collation** - `album_artist_flow/collation.rs` (unify mixed-artist albums to "Various Artists")
3. **Album Artist Population** - `album_artist_flow/population.rs` (bulk-fill missing album_artist)

### Remaining Work

- Actual tag writes on commit (currently logs only) - `mod.rs:2053`
- ~~Collation/Population review flows~~ - DONE (review states with commit handlers)

---

## Album Tag Resolution [IMPLEMENTED]

> Implemented in `src/ui/album_flow/` with EP/edition detection.

- Album canonicalization with EP/LP/edition variant detection
- Uses `album_normalization.rs` for format/edition parsing
- Flags buckets with format variants for metadata-duplicate review

### Remaining Work

None - album flow is complete.

---

## Corpus Organization

### Directory Mapping
- Map corpus directories to external source identifiers
- Example: `web/releases/bandcamp/artist` -> MusicBrainz artist ID
- Enable automated metadata lookup by directory structure
- Record label + catalog number as preferred canonical scheme

### Deployment Path Structuring
- Nuanced path structuring for vocalist/remix placement
- Configurable via "Opinions" in config.kdl
- Drive dialogue to establish operator's consistent preferences
- Handle compilation albums intelligently

### Intake Workflow
- Scanning and importing external material into corpus
- Automatic directory structure suggestions
- Duplicate detection during import
- Quarantine area for review

---

## Deduplication Workflow

### UI Integration (Phase 5 of FINGERPRINT_DEDUP_INTEGRATION.md)
- Menu state integration for cluster-based resolution
- Picker result handling and terminal management
- Statistics display with auto-removal metrics

### Testing & Validation (Phase 6)
- Test auto-removal for bitrate-only differences
- Test handling of missing bitrate data
- Test multi-cluster non-transitive relationships
- Test auto-ignore pattern detection
- Test picker navigation

### Polish (Phase 7)
- Inline comments for complex sections
- README updates with new workflow
- User-facing documentation

### Additional Enhancements
- Progress indicators for large auto-removal operations
- Parallel processing with rayon for auto-removal
- Enhanced conflict visualization (file paths, bitrate comparison, file size)

---

## UI Improvements

### Corpus Browser [PARTIALLY IMPLEMENTED]

> Basic implementation in `src/ui/corpus_browser/` with two-pane layout (directory tree + metadata preview).

Implemented:
- Directory tree navigation with track counts
- Metadata preview pane (bitrate, duration, sample rate, tags)
- Enter on directory/file to open tag editor

Remaining:
- ~~Tag editor track loading from corpus browser~~ - DONE (`start_tag_editor_for_path()`)
- Table view with sortable columns
- Filter/search by metadata field
- Quick statistics view

### First-Time Setup Flow
- Detect missing config file on startup
- Interactive wizard to set initial config values:
  - Corpus root directory
  - Library paths
  - Optional legacy library path
- Write generated config.kdl to appropriate XDG location

### Tag Editor [PARTIALLY IMPROVED]

Implemented:
- Edit indicator (pencil icon) for modified fields
- Action pane with Proceed button (replaced search pane)

Remaining:
- ~~Actual tag saving to disk~~ - DONE (`save_tag_editor_changes()` with `metadata::write_tags()`)
- Search functionality within tags
- Better multi-value tag handling (multiple album_artist entries)
- Batch operations across all loaded tracks

### Modal System
- Proper modal dialogue handling (currently stubbed)
- Confirmation dialogs with consistent styling
- Progress modals for long operations

### Pending Changes View
- Full visualization of queued changes
- Change grouping and filtering
- Commit/revert individual change sets
- Change preview before commit

### Reports
- Fill in report descriptions (currently TODO placeholders):
  - "Generate all configured reports"
  - "Report on legacy library coverage"
  - "Report on deployment status"
  - "Report on metadata quality"
  - "Report on detected duplicates"

### Decision Flow
- ~~Count actual pending changes (currently hardcoded to 0)~~ - DONE
- Metadata conflict resolution across duplicate tracks
- Audio fingerprint duplicate resolution

### Deployment
- Preview deployment without making changes
- Create hard links in library directories
- Review queued changes before commit

---

## Runtime Improvements

### Re-fingerprint Support
- Pass re_fingerprint flag to scanner to clear scan_state cache
- Allow forcing fingerprint regeneration on demand

### Tag Saving
- Implement actual tag saving (currently stubbed)
- Write changes to audio file metadata
- Support both in-place and copy-on-write modes

### Change Commit
- Implement change commit functionality (currently stubbed)
- Execute pending changes atomically
- Generate commit reports

---

## Philosophy Alignment

From PHILOSOPHY.md "misc notes":

### Scan-Time Insights
- Compute insights and reports during scans
- Display latest report summaries in menu
- Avoid requiring separate generation step

### Workflow Nudging
- Upon scan completion, generate/finalize reports
- Surface health issue summaries
- Prompt user to enter relevant resolution flows

### Library Health Concepts
- Enumerate additional health metrics to track
- Define thresholds and severity levels
- Create dashboard view of corpus health

---

## Technical Debt

This section consolidates all TODO comments from the codebase. Keep this synchronized when adding or resolving TODOs in code.

*Last updated: 2026-01-12*

### UI Stubs

| Location | Description | Status |
|----------|-------------|--------|
| `mod.rs:888` | Pending changes view stub | Open |
| ~~`mod.rs:1068-1072`~~ | ~~Corpus browser → tag editor track loading~~ | DONE |
| `mod.rs:1209-1210` | Modal display handling stub | In Progress |
| `mod.rs:1240-1271` | Tag editor commit flow (save to disk, stale deployment check) | DONE |
| ~~`mod.rs:1289`~~ | ~~Change commit stub~~ | DONE |
| ~~`mod.rs:1493`~~ | ~~Export change list stub~~ | DONE |
| ~~`dialogue.rs:231`~~ | ~~Pending changes count hardcoded to 0~~ | DONE |

### Flow Commits (Tag Changes Not Written to Disk)

| Location | Description | Status |
|----------|-------------|--------|
| `mod.rs:2053` | Album artist canonicalization commit (logs only) | DONE |
| ~~`mod.rs:2121`~~ | ~~Collation review flow~~ | DONE |
| ~~`mod.rs:2147`~~ | ~~Population review flow~~ | DONE |

### Health System

| Location | Description | Status |
|----------|-------------|--------|
| ~~`corpus/health/detection.rs:5`~~ | ~~Out-of-band tag change detection module~~ | DONE |
| ~~`corpus/health/heartbeat.rs:12`~~ | ~~Health warnings system~~ | DONE |
| `canon_flow/mod.rs:40` | Health check: tags mismatching on-disk vs in-index | Open |
| `canon_flow/session.rs:140` | Health check: tags mismatching on-disk vs in-index | Open |
| `db/changes.rs:49` | OutOfBandTagChange health check resolution UI | Open |
| `ops/changes.rs:379` | Operations flow for resolving OutOfBandTagChange mutations | Open |
| `corpus/health/detection.rs` | Artist/album_artist canonicalization mismatch detection | Open |

### Database Query Patterns

| Location | Description | Status |
|----------|-------------|--------|
| `db/queries/*.rs` | 17-column SELECT for row_to_track duplicated across files | Open |

### Metadata Duplicate Flow

| Location | Description | Status |
|----------|-------------|--------|
| `ui/main_menu.rs:734-745` | Flow stub - needs redesign to use DeployConflicts | Open |
| `ops/reports.rs:1109` | `populate_metadata_duplicate_groups` - candidate for removal | Open |
| `ops/reports.rs:528` | Call site for metadata duplicate population - candidate for removal | Open |

### Opinions System

| Location | Description |
|----------|-------------|
| `ui/app.rs:187` | Resolve coin-flip actions to Opinion in the future |

### Menu Descriptions

~~Items in `src/ui/main_menu.rs` with "TODO:" descriptions~~ - **ALL DONE**

All menu descriptions have been updated with meaningful text.

---

## Notes

- This document is a loose collection of ideas, not commitments
- Features should be prioritized based on actual usage patterns
- Some items may be superseded by better approaches
- Extracted from CLAUDE.md, FINGERPRINT_DEDUP_INTEGRATION.md, and TODO comments
