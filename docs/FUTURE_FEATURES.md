# MLA Future Features

This document catalogs planned features and improvements for MLA. Items are loosely categorized and should be considered a backlog rather than a roadmap.

---

## Database & Storage

### Database Snapshotting & Restoring

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

## Corpus Organization

### Directory Mapping
- Map corpus directories to external source identifiers
- Example: `web/releases/bandcamp/artist` -> MusicBrainz artist ID
- Enable automated metadata lookup by directory structure

### Deployment Path Structuring
- Nuanced path structuring for vocalist/remix placement
- Configurable via "Opinions" in config.kdl

### Intake Workflow
- Scanning and importing external material into corpus
- Automatic directory structure suggestions
- Duplicate detection during import

---

### First-Time Setup Flow [PARTIALLY IMPLEMENTED]

Implemented:
- Auto-detect empty database on startup
- Automatically start corpus scan with progress display
- Heartbeat now handles ongoing scanning of new files

Remaining:
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
- Multi-value tag support with `[N values]` display and popup editor
- Name/value focus indicator (separate highlighting)
- Delete/strikethrough for tag deletion
- Tag rename detection

Remaining:
- Centralized controls panel: Design a ControlsContext trait or similar that each UiMode/modal can implement to provide context-sensitive controls to the ever-present bottom panel (`src/ui/tag_editor/state.rs:979`)

---

## Notes

- This document is a loose collection of ideas, not commitments
