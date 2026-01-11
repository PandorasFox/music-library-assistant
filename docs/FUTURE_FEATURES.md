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

### Code Locations Needing Work

- `src/ui/mod.rs:170` - Deploy functionality stub
- `src/ui/mod.rs:179` - Re-fingerprint flag not passed to scanner
- `src/ui/mod.rs:301` - Pending changes view stub
- `src/ui/mod.rs:444-445` - Modal handling stub
- `src/ui/mod.rs:454-455` - Tag saving stub
- `src/ui/mod.rs:470` - Change commit stub
- `src/ui/dialogue.rs:230` - Pending changes count hardcoded
- `src/ui/tag_editor.rs:868` - Search panel stub

### Menu Descriptions
All items in `src/ui/main_menu.rs` with "TODO:" descriptions need proper explanatory text.

---

## Notes

- This document is a loose collection of ideas, not commitments
- Features should be prioritized based on actual usage patterns
- Some items may be superseded by better approaches
- Extracted from CLAUDE.md, FINGERPRINT_DEDUP_INTEGRATION.md, and TODO comments
