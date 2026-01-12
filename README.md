# Music Library Assistant (MLA)

A high-performance music library management tool for managing large music collections with multiple organizational strategies. Named after the Music Library Assistant from The Talos Principle.

**Repository**: https://git.hecate.pink/hecate/mla

## Installation

```bash
cargo install --git 'https://git.hecate.pink/hecate/mla'
```

This installs the `mla` binary to your Cargo bin directory (typically `~/.cargo/bin/`). Ensure this is in your PATH.

### Building from source

```bash
git clone 'https://git.hecate.pink/hecate/mla'
cd mla
cargo build --release
```

The binary will be at `target/release/mla`.

## Configuration

MLA requires a config file at `$XDG_CONFIG_HOME/mla/config.kdl` (or `~/.config/mla/config.kdl`):

```kdl
// Root directory of your music archive
corpus-root "/path/to/your/music/archive"

// Define your deployed libraries
library "main" {
    path "/path/to/your/library"
}

// Optional: Path to a legacy library that needs organization
legacy-library "/path/to/legacy/music"
```

See `config.kdl.example` for a complete example with additional options.

## Usage

Launch the TUI:

```bash
mla
```

### Navigation

- **Arrow keys** (up/down): Navigate menus
- **Enter**: Select item
- **ESC**: Go back
- **Q**: Quit (from main menu)

### Menu Categories

- **Insight & Health**: Corpus health dashboard, analysis reports
- **Intake**: Scan external sources, import from legacy library
- **Organization**: Deduplication, tag repairs, file consolidation
- **Deployment**: Preview changes, deploy to libraries, view pending changes
- **Operations**: Rescanning, database operations

## Features

- Fast metadata extraction (FLAC, MP3, OGG, M4A, and more)
- SQLite-based indexing for quick queries
- Interactive TUI with keyboard navigation
- Multiple report types (legacy matching, deployment status, quality issues, duplicates)
- Inode-based hard-link detection
- Acoustic fingerprinting for duplicate detection
- Read-only operations preserve archive integrity

## Librarian Workflow Philosophy

MLA organizes library management around classic librarian cycles:

1. **Insight & Health** - Understanding corpus state, metadata quality, deployment coverage
2. **Intake** - Bringing external material into corpus, normalizing metadata
3. **Organization** - Corpus-mutating operations: deduplication, tag repairs, consolidation
4. **Deployment** - Publishing corpus to browsable libraries via hard links
5. **Operations** - Low-level maintenance, rescanning, database operations

All corpus-mutating operations are tracked as composable, reversible algebraic changes that accumulate before execution. This enables confident experimentation with large-scale organizational changes.

## Documentation

- [Philosophy](docs/PHILOSOPHY.md) - Design principles and conceptual foundation
- [Future Features](docs/FUTURE_FEATURES.md) - Planned features and backlog
- [Development Status](docs/DEVELOPMENT.md) - Implementation status and technical details

## License

To be determined.
