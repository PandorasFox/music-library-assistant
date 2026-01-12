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

## Album Artist Health Restoration

A unified meta-flow for resolving album_artist issues across the corpus. This is critical for proper deployment path computation since deployment paths use album_artist.

### Three Sub-Flows

1. **Album Artist Canonicalization**
   - Similar to Artist Name Canonicalization flow
   - Groups album_artist values by normalized form (case-insensitive)
   - Presents buckets of variants for squashing to canonical form
   - Example: "Dragonforce" vs "DragonForce" vs "DRAGONFORCE"

2. **Album Artist Inference (Album Patterns)**
   - Detects tracks that likely belong to the same album but have no album_artist
   - Groups by: similar album name + presence of track numbers + similar artist
   - Presents candidates for album_artist inference
   - Requires new health metrics and computations

3. **Album Artist Population**
   - Tracks with album + artist but no explicit album_artist
   - Simple case: copy artist to album_artist
   - May need operator confirmation for compilation albums

### Design Requirements

- **Transformed State Reasoning**: Each sub-flow should see the corpus as it WILL BE after earlier flows' mutations are applied. Use virtual overlay of pending mutations.

- **Progressive Resolution**: Prevent redundant work by computing metrics against the in-memory transformed state.

- **3-Pane/3-Stage Review**: Final review showing all three categories of changes before commit. Each pane independently reviewable.

- **Final Resolution Display**: Summary showing overall transformed state, how many tracks affected, post-mutation album_artist distribution.

### Health Metrics Needed

- Album artist capitalization variant buckets (group by normalized form)
- Tracks with album + track_number but missing album_artist (grouped by album similarity)
- Tracks with album + artist but missing album_artist
- Album coherence score (do all tracks in an "album" agree on album_artist?)

### Implementation Notes

- Share mutation accumulation infrastructure with canon_flow
- Virtual corpus state: HashMap<track_id, PendingChange> overlay
- Review panes can reuse CanonSessionReview patterns
- Consider "back" navigation between flows (not just within)
- Menu entries stubbed in `src/ui/main_menu.rs`

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

### Corpus Catalogue Browser (TABLE STAKES)
- TUI interface to browse and search the indexed corpus
- Table view with sortable columns (artist, album, title, path, bitrate, etc.)
- Filter/search by any metadata field
- Navigate to tag editor for selected track(s)
- Quick statistics view (total tracks, artists, albums, bitrate distribution)
- This is fundamental functionality that should exist

### First-Time Setup Flow
- Detect missing config file on startup
- Interactive wizard to set initial config values:
  - Corpus root directory
  - Library paths
  - Optional legacy library path
- Write generated config.kdl to appropriate XDG location

### Tag Editor
- Search functionality within tags (currently shows "(TODO)")
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
- Count actual pending changes (currently hardcoded to 0)
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

### UI Stubs

| Location | Description |
|----------|-------------|
| `src/ui/mod.rs:850` | Pending changes view stub |
| `src/ui/mod.rs:1122-1123` | Modal display handling stub |
| `src/ui/mod.rs:1132-1133` | Tag saving stub |
| `src/ui/mod.rs:1148` | Change commit stub |
| `src/ui/mod.rs:1352` | Export change list stub |
| `src/ui/tag_editor/render.rs:212` | Search panel shows "(TODO)" |
| `src/ui/dialogue.rs:231` | Pending changes count hardcoded to 0 |

### Health System

| Location | Description |
|----------|-------------|
| `src/health/detection.rs:5` | Out-of-band tag change detection module |
| `src/health/heartbeat.rs:12` | Health warnings system |
| `src/ui/canon_flow/mod.rs:40` | Health check: tags mismatching on-disk vs in-index |
| `src/ui/canon_flow/session.rs:140` | Health check: tags mismatching on-disk vs in-index |
| `src/db/changes.rs:49` | OutOfBandTagChange health check resolution UI |
| `src/ops/changes.rs:379` | Operations flow for resolving OutOfBandTagChange mutations |

### Opinions System

| Location | Description |
|----------|-------------|
| `src/ui/app.rs:172` | Resolve coin-flip actions to Opinion in the future |

### Menu Descriptions

All items in `src/ui/main_menu.rs` with "TODO:" descriptions need proper explanatory text:

| Location | Item |
|----------|------|
| `src/ui/main_menu.rs:566` | "Generate all configured reports" |
| `src/ui/main_menu.rs:573` | "Report on legacy library coverage" |
| `src/ui/main_menu.rs:580` | "Report on deployment status" |
| `src/ui/main_menu.rs:587` | "Report on metadata quality" |
| `src/ui/main_menu.rs:594` | "Report on detected duplicates" |
| `src/ui/main_menu.rs:607` | "Resolve metadata conflicts across duplicate tracks" |
| `src/ui/main_menu.rs:637` | "Review queued changes before commit" |
| `src/ui/main_menu.rs:649` | "Import external material into corpus" |

---

## Notes

- This document is a loose collection of ideas, not commitments
- Features should be prioritized based on actual usage patterns
- Some items may be superseded by better approaches
- Extracted from CLAUDE.md, FINGERPRINT_DEDUP_INTEGRATION.md, and TODO comments
