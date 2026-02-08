# Music Library Assistant (MLA)

A high-performance music library management tool for managing large music collections.

I recommend organizing your music by acquisition sources (physical/digital, bandcamp/itunes/qobuz, etc) for ease of long-term organization scaling - a basic "library/albumartist/album/track" structure scales poorly when you have thousands of distinct album artists!

If you still want a flat albumartist/album/track library-presentation of your music corpus (consistent presentation of files, downstream music players or servers), MLA offers this functionality with its hard-link deployment model. I'll write up an organization_strategies.md doc later, I prommy.

**Repository**: https://git.hecate.pink/hecate/mla

## Screenshots (beta edition)
<img width="300" height="" alt="Screenshot 2026-02-07 at 00 38 12" src="https://github.com/user-attachments/assets/15efffd4-13ca-4b99-b037-2be6c7291dc6" />
<img width="300" height="" alt="Screenshot 2026-02-07 at 16 37 21" src="https://github.com/user-attachments/assets/f4c76bf0-8600-4147-b88f-8f1ec5231009" />
<img width="300" height="" alt="Screenshot 2026-02-07 at 16 37 48" src="https://github.com/user-attachments/assets/b444d872-819d-4799-85e1-adf77f9643c4" />
<img width="300" height="" alt="Screenshot 2026-02-07 at 16 37 56" src="https://github.com/user-attachments/assets/199a4492-3042-4111-8517-6335edaf30ce" />
<img width="300" height="" alt="Screenshot 2026-02-07 at 16 38 03" src="https://github.com/user-attachments/assets/b65d35c4-182c-45ed-98e9-082c5e387ac6" />
<img width="300" height="" alt="Screenshot 2026-02-04 at 20 15 01" src="https://github.com/user-attachments/assets/ee8c1764-e298-4457-8dcc-f2a1651e831e" />

Please note that the UI (positioning, text-wrapping, etc) is very unpolished because this is still somewhere vaguely in beta territory. The focus of development has been around safety guarantees of performing tag edits and ensuring index/file consistency.

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

All corpus-mutating operations are tracked as composable, reversible algebraic changes that accumulate before execution. All tag edits are also logged to an edit history table. Reversing edits & built-in DB snapshotting/roll-backing are planned to be supported, eventually.

All corpus-mutation operations *must* be confirmed via a user's Enter keypress. This is enforced at compile-time thanks to some clever Rust sealed trait usage.

## Disclaimer

This project is approximately 99% codegenned (with Claude). I review and test all changes, broadly. My attitude is that my threshold for bugs is "minor UI jank", and underlying Systems must be ironclad and well-reviewed and tested for me to ship them. UI State itself is still very critical, as it's authoritative to what gets sent to the underlying Mutation-applying systems, but I am an infrastructure engineer that deeply does not want to handle all the TUI modal/state/input handling, nor hand-write all of the hundreds of small SQL queries needed for the sqlite operations necessary.

That being said: I still designed this software at the systems layer with this all in mind, and the software itself is designed to not do anything more dangerous than editing tags. I do not let the codebase have the concept of 'removing a file', and it can only move files at most. All mutations (to the Corpus or Corpus Index) must first be staged to the underlying Transaction, and then the Transaction itself must be reviewed and confirmed before any changes will be made. This _is_ enforced via sealed-trait "callsite witnesses" at compile-time, which enforces strong barriers between the underlying Mutation engine and its code, and the UI code.

This is still 'unreleased' software, in that I haven't felt it appropriate to cut a release yet while I'm still fleshing out features (and I haven't dedicated any thought to what license to slap on this yet). I had a couple incidents during development around not using DB transactions and mis-using tag-editing interfaces and _did_ drop some tags (including album art) from some source files, and I've since regenerated all of my (working copy of my) corpus from source archives. I've since ironed out _all_ of the edge cases in tag-editing and general inode-touching.

## Other Notes

my DB file is about 600MiB for ~25,000 files that take up ~700GiB on-disk. This includes a fair amount of binary chromaprint data, as well as some number of ephemeral signal data for health issues I've yet to resolve in my onw music library because I'm still working on the resolution flows, as well as a *lot* of development tag-edit tests.

600MiB sqlite db file might sound like a lot, but this is still only 0.1% as large as all of the music files I have themselves. The db file is stored in `~/.local/share/mla/mla.db` (or whatever the appropriate XDG prefix is, if the value is set, i think - i know i did that for XDG_CONFIG_HOME....) so that it can be located on flash storage instead of spinning rust.

## License

To be determined.
