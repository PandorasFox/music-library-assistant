# Music Library Assistant (MLA)

MLA is a high-performance music library management tool designed to help you manage large music corpus with multiple organizational strategies. Named after the Music Library Assistant from The Talos Principle, it provides comprehensive reporting and analysis capabilities to help you make informed decisions about your music collection.

## Features

- Fast metadata extraction from audio files (FLAC, MP3, OGG, M4A, and more)
- SQLite-based indexing for quick queries
- Interactive TUI with keyboard navigation
- Multiple report types:
  - Legacy library matching (find definitive matches in your corpus)
  - Corpus deployment status (track which files are deployed to libraries)
  - Quality reports (canonicalization issues, missing tags)
  - Duplicate detection (across different bitrates and locations)
- Inode-based hard-link detection
- Read-only operations preserve archive integrity

## Installation

### Building from source

```bash
cargo build --release
```

The binary will be located at `target/release/mla`.

You can optionally copy it to your PATH:
```bash
cp target/release/mla ~/.local/bin/
```

## Configuration

MLA uses a `config.kdl` file for configuration. The file should be placed at `$XDG_CONFIG_HOME/mla/config.kdl` (or `~/.config/mla/config.kdl` if `XDG_CONFIG_HOME` is not set):

```kdl
// Root directory of your music archive
corpus-root "/Volumes/cerberus/archive/music"

// Define your deployed libraries
library "main" {
    path "/Volumes/cerberus/library/main"
}

library "soundtracks" {
    path "/Volumes/cerberus/library/soundtracks"
}

// Optional: Path to your legacy library that needs organization
legacy-library "/Volumes/cerberus/archive/working/legacy"
```

See `config.kdl.example` for a complete example.

## Usage

MLA is a TUI (Terminal User Interface) application. Launch it by running:

```bash
mla
```

### Navigation

- **Arrow keys** (↑/↓): Navigate menu
- **Enter**: Select highlighted item
- **ESC**: Go back to previous menu
- **Q**: Quit (from main menu)

### Main Menu Options

- **Scan Corpus/Library**: Scan audio files and build the database index
- **Generate Reports**: Create analysis reports (legacy matches, deployment status, quality issues, duplicates)
- **Deploy to Libraries**: Deploy corpus files to libraries via hard links
- **Duplicate Resolution**: Tag editor for resolving duplicate tracks (demo)
- **Quit**: Exit MLA

### Reports

All reports are generated in `/tmp` with the prefix `mla-`:

#### Legacy Library Report (`/tmp/mla-legacy_matches.txt`)

Finds files in your legacy library that definitively exist in your corpus.

**Match confidence levels:**
- **High**: Exact metadata match (artist, album, title) with optional duration confirmation
- **Medium**: Reserved for future use
- **Low**: Duration and file type match (tags may differ)

The report includes:
- Summary statistics
- Detailed matches with confidence levels
- Parsable format section (pipe-delimited, high-confidence matches only)

**Use case:** Helps identify which legacy files can be safely removed after validation.

#### Deployment Status Report (`/tmp/mla-deployment_status.txt`)

Shows which archive directories are deployed to libraries via hard links.

**Status types:**
- **FULLY DEPLOYED**: All files in directory are deployed
- **PARTIALLY DEPLOYED**: Some files deployed, some not
- **NOT DEPLOYED**: No files from this directory are in any library

**Use case:** Find undeployed material, identify forgotten albums.

#### Quality Report (`/tmp/mla-quality_issues.txt`)

Identifies metadata quality issues:

1. **Artist Name Canonicalization**: Different capitalizations/spellings of the same artist
   - Example: "deadmau5" vs "Deadmau5" vs "deadmau5."

2. **Missing Album Artist Tags**: Tracks missing album_artist tags (causes poor library organization)

**Use case:** Clean up inconsistent tagging before deploying to libraries.

#### Duplicate Detection Report (`/tmp/mla-duplicates.txt`)

Finds duplicate tracks across your corpus and libraries.

**Detection strategy:**
- Matches by normalized artist, album, and title
- Excludes remixes and deluxe editions (basic heuristic)
- Shows bitrate and file format for each instance
- Helps identify same content at different quality levels

**Use case:** Decide which versions to keep when you have the same track in multiple bitrates/formats.

## Architecture

### Organization Strategy

MLA is designed around the concept of:
1. **Archive**: Source files organized by logical acquisition source
2. **Libraries**: Deployed files (via hard links) organized by artist/album
3. **Legacy**: Disorganized files being validated and cleaned up

### Database Schema

MLA stores metadata in SQLite (at `$XDG_CONFIG_HOME/mla/mla.db` or `~/.config/mla/mla.db`):
- File path, inode, size, format
- Audio metadata: artist, album, album_artist, title, track number
- Technical data: duration, bitrate, sample rate
- Source tracking (which scan produced this entry)

### Hard Link Detection

Libraries deployed via hard links from the corpus share the same inode. MLA uses this to:
- Determine deployment status
- Avoid false duplicates (hard links aren't duplicates)
- Track which archive files are actively used

## Development Status

### Implemented
- ✅ Core scanning and metadata extraction
- ✅ Interactive TUI with keyboard navigation
- ✅ Legacy library matching reports
- ✅ Deployment status reports
- ✅ Quality/canonicalization reports
- ✅ Basic duplicate detection

### Future Enhancements
- Acoustic fingerprinting (Chromaprint) for more robust duplicate detection
- Config-driven scanning (scan all configured sources with one command)
- More sophisticated remix/edition detection
- Report filtering and customization options
- Integration with external tools (beets, etc.)

## Technical Details

- **Language**: Rust
- **Dependencies**:
  - `symphonia`: Audio metadata extraction
  - `ratatui` + `crossterm`: Terminal UI
  - `rusqlite`: Database
  - `kdl`: Config parsing
- **Performance**: Optimized for large collections (10,000+ files)
- **Platform**: Unix-like systems (uses inode for hard link detection)

## Philosophy

MLA is **read-only by design** for corpus analysis. It never modifies your corpus files without explicit action. The core operations are:
- Scanning and indexing
- Analysis and reporting
- Providing information for **you** to make decisions

Deployment operations use hard links to safely reference corpus files in libraries without duplication.

## License

To be determined.

## Contributing

This is a personal tool, but suggestions and improvements are welcome!
