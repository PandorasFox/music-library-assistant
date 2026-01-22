# Music Library Assistant (MLA)

A high-performance music library management tool for managing large music collections with multiple organizational strategies.

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

There will, eventually, be a built-in config editor + first-time setup wizard. For now, though: woe, config file be upon ye.

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

## Features

- Interactive TUI with keyboard navigation
- Multiple report types (legacy matching, deployment status, quality issues, duplicates)
- Acoustic fingerprinting for duplicate detection
- Algebraic Mutation and Batch Execution systems for staging, previewing, and bulk-applying changes!

All corpus-mutating operations are tracked as composable, reversible algebraic changes that accumulate before execution.

All corpus-mutation operations *must* be confirmed via a user's Enter keypress. This is enforced at compile-time thanks to some clever Rust sealed trait usage.

This enables confident experimentation with large-scale organizational changes.

## Disclaimer

This project is approximately 99% codegenned (with Claude). I review all changes as best I can, but I also still code-churned thousands of lines of code in the prototyping stage.

That being said: I still designed this software at the systems layer with this all in mind, and the software itself is designed to not do anything more dangerous than editing tags. I do not let the codebase have the concept of 'removing a file', and it can only move files at most.

## Documentation

- [Philosophy](docs/PHILOSOPHY.md) - Design principles and conceptual foundation
- [Future Features](docs/FUTURE_FEATURES.md) - Planned features and backlog
- [Development Status](docs/DEVELOPMENT.md) - Implementation status and technical details

## License

To be determined.
