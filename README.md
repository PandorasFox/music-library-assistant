# Music Magic (MM)

A high-performance music library management tool for managing large music collections.

I recommend organizing your music by acquisition sources (physical/digital, bandcamp/itunes/qobuz, etc) for ease of long-term organization scaling - a basic "library/albumartist/album/track" structure scales poorly when you have thousands of distinct album artists!

If you still want a flat albumartist/album/track library-presentation of your music corpus (consistent presentation of files, downstream music players or servers), MM offers this functionality with its hard-link deployment model. I'll write up an organization_strategies.md doc later, I prommy.

**Repository**: https://git.hecate.pink/hecate/mla

## Screenshots

recording soon(tm)

## Installation

```bash
cargo install --git 'https://git.hecate.pink/hecate/mm'
```

This installs the `mm` binary to your Cargo bin directory (typically `~/.cargo/bin/`). Ensure this is in your PATH.

### Building from source

```bash
git clone 'https://git.hecate.pink/hecate/mm'
cd mm
cargo build --release
```

The binary will be at `target/release/mm`.

## Configuration

MM requires a config file at `$XDG_CONFIG_HOME/mm/config.kdl` (or `~/.config/mm/config.kdl`):

There will, eventually, be a built-in config editor + first-time setup wizard. For now, though: woe, config file be upon ye.

```kdl
// Archive root - corpus/, libraries/, and stash/ are derived subdirectories
root "/path/to/your/archive"

// Define source directories and their library deployment targets
// Paths are relative to <root>/corpus/
dir "web/releases/bandcamp" {
    library "music"
}

dir "physical/vinyls" {
    library "music"
}

// Configure behavior (all have sensible defaults)
opinions {
    // Capture lossy formats (MP3, M4A, etc.) to FLAC instead of transcoding to Opus
    // lossy-shit-formats-to-flac true

    startup {
        // force-check-all-files false
    }

    fingerprint-matching {
        // duration-tolerance-percent 10.0
    }
}
```

See `config.kdl.example` for a complete example with additional options.

## Usage

Launch the TUI:

```bash
mm
```

## Features

- Interactive TUI with keyboard navigation
- Multiple report types (legacy matching, deployment status, quality issues, duplicates)
- Acoustic fingerprinting for duplicate detection
- Algebraic Mutation and Batch Execution systems for staging, previewing, and bulk-applying changes!

All corpus-mutating operations are tracked as composable, reversible algebraic changes that accumulate before execution. All tag edits are also logged to an edit history table. Reversing edits & built-in DB snapshotting/roll-backing are planned to be supported, eventually.

All corpus-mutation operations *must* be confirmed via a user's Enter keypress. This is enforced at compile-time thanks to some clever Rust sealed trait usage.

## Disclaimer & Architecture

I leverage claude for this project because I have mild dyslexia and struggle with the writing-side - I can and have written thousands of lines of Rust before, but it melts my brain. Doing rigorous design work followed by code review, testing, and integration/cleanup verification is necessary in all of this, but I've still found it to ultimately be more effective than hand-writing everything. Writing broken code and throwing it out is something I can both do by hand, and with a tool, with the main difference being where in the process I spend my time.

After all, the problem spaces of "reason about metadata tags, calculate levenshtein distances, apply bulk compute" are both largely solved, and things I can do in my sleep after 5 years of youtube cdn ops work.

This software has been designed at the systems level alongside rust's type system with this in mind:

- sealed trait 'witnesses' to User enter keypresses, ensuring all mutating operations have routed through an enter keypress dispatch
  - This is a really fun compile-time guarantee that all operation-driving logic is contained in the UI, and that _only_ operation decisions can drive change.
- all changes are modelled as Composable Mutations
  - e.g. a tag edit is roughly `edit(inode, tag_name, old_value, new_value)` - we can accumulate, and *MUST* review, these before committing them to db and/or disk.
  - changes get aggregated by inode when scheduling bulk computation :)
  - avoids many problems with trying to apply edits in real-time
- a batch Mutation and Computation engine (or insights pipelines, whatever you want to call it) for parallelizing file tag edits and tag cloud analysis
  - worker threads have their own read-only DB connection for reads
  - _only_ worker threads have Send endpoints for the DB write thread's queue
  - mutations & computations can chain-emit follow-on operations (e.g. EditTagDb can emit FlushTagsToDisk), with in-band signalling to prevent DB thread write queue race conditions
- meta-level types for inter-system logic like Mutations, Computations, and Signals so that they can own their own DB schema for consistency across UI and infra
- multi-threaded application logic using an orchestrator thread that owns the work scheduling interface
  - UI thread for input polling/queueing work into transactions -> confirming transactions only with enter keypresses (or mouse clicks!)
  - rayon worker thread pool (configurable size)
  - DB write thread to avoid write lock contention
- files are 'stashed' to an out-dir when being removed. Destroying data is an operator action.

With all the systems in place, development basically boils down to:
- design a Signal (fingerprint duplicates, missing tags, inconsistent album_artist in compilation albums,....)
  - what heuristics do we use to emit this signal?
- design a computation that emits that signal
- design a mutation to resolve that signal (if necessary; tag editing covers most cases)
- glue together the collection of Signals with some basic actions that the user can review and apply easily

This software is provided 'as-is', and is still unlicensed as an 'initial release' has still yet to be cut.

## Other Notes

my DB file is about 600MiB for ~25,000 files that take up ~700GiB on-disk. This includes a fair amount of binary chromaprint data, as well as some number of ephemeral signal data for health issues I've yet to resolve in my own music library because I'm still working on the resolution modals, as well as a *lot* of development tag-edit tests.

600MiB sqlite db file might sound like a lot, but this is still only 0.1% as large as all of the music files I have themselves. The db file is stored in `~/.local/share/mm/mm.db` (or whatever the appropriate XDG prefix is, if the value is set, i think - i know i did that for XDG_CONFIG_HOME....) so that it can be located on flash storage instead of spinning rust.

## License

To be determined.
