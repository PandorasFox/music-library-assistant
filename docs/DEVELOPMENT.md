# Development Status

> **Work In Progress**
> This is personal/experimental software under active development.
> No backwards compatibility guaranteed. Database schema, config format,
> and structures may change without notice.

## Implementation Status

### Implemented
- Core scanning and metadata extraction
- Interactive TUI with keyboard navigation
- Legacy library matching reports
- Deployment status reports
- Quality/canonicalization reports
- Basic duplicate detection
- Acoustic fingerprinting (Chromaprint) for robust duplicate detection
- Fingerprint-based deduplication workflow with auto-removal
- Incremental scanning (inode + mtime based)
- Config-driven scanning (scan all sources at once)

### In Progress
- Algebraic change tracking (pending change accumulation)
- TUI reorganization around librarian workflows
- Staging preview for changes

### Future Considerations
- Database snapshotting and metadata backup/restore
- External metadata source integration (MusicBrainz, Discogs)
- Confidence levels for metadata sources
- Corpus directory → external source key mapping

## Technical Details

- **Language**: Rust
- **Dependencies**:
  - `symphonia`: Audio metadata extraction
  - `ratatui` + `crossterm`: Terminal UI
  - `rusqlite`: Database
  - `kdl`: Config parsing
- **Performance**: Optimized for large collections (10,000+ files)
- **Platform**: Unix-like systems (uses inode for hard link detection)

## Architecture

### Organization Strategy

MLA is designed around the concept of:
1. **Archive**: Source files organized by logical acquisition source
2. **Libraries**: Deployed files (via hard links) organized by artist/album
3. **Legacy**: Disorganized files being validated and cleaned up

### Database Schema

MLA stores metadata in SQLite (at `$XDG_DATA_HOME/mla/mla.db` or `~/.local/share/mla/mla.db`):
- File path, inode, size, format
- Audio metadata: artist, album, album_artist, title, track number
- Technical data: duration, bitrate, sample rate
- Source tracking (which scan produced this entry)

### Hard Link Detection

Libraries deployed via hard links from the corpus share the same inode. MLA uses this to:
- Determine deployment status
- Avoid false duplicates (hard links aren't duplicates)
- Track which archive files are actively used
