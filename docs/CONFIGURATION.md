# Configuration Reference

## Overview

MM uses [KDL](https://kdl.dev) for configuration files, located in your XDG config directory:

- **Primary**: `$XDG_CONFIG_HOME/mm/config.kdl` (typically `~/.config/mm/config.kdl`)
- **Source directories**: `$XDG_CONFIG_HOME/mm/dirs.kdl` (optional, separate file)

Both files must exist for MM to start (config.kdl is mandatory, dirs.kdl is optional — an empty source dirs list is valid). See `config.kdl.example` in the repository root for a commented example.

Environment variables can override any config value — see [Environment Variable Overrides](#environment-variable-overrides) below.

## Quick Start

Minimal `config.kdl`:

```kdl
root "/path/to/your/archive"
```

This gets you running with all defaults. The archive root must contain `corpus/`, `libraries/`, `stash/`, and `inbox/` subdirectories, all on the same filesystem.

## Archive Root

```kdl
root "/Volumes/cerberus/archive"
```

The `root` directive is the only mandatory field. MM derives four subdirectories from it:

| Directory | Purpose |
|-----------|---------|
| `<root>/corpus/` | Source-of-truth audio files, organized by source directories |
| `<root>/libraries/` | Deployment targets (e.g., Navidrome media dirs) — hardlinked from corpus |
| `<root>/stash/` | Quarantine for resolved duplicates |
| `<root>/inbox/` | Drop zone for new files awaiting organization |

**Validation**: All four must exist and reside on the same filesystem (required for hardlinking).

## Legacy Library Mode

```kdl
legacy-library true
```

Optional. Enables dissection of an existing tag-organized library. Default: `false`.

## Source Directories (`dirs.kdl`)

Source directories define logical collections within the corpus. They control library deployment targets and per-directory behavior overrides.

```kdl
dir "web/releases/bandcamp" {
    library "music"
    library "soundtracks"
    can-stash-dupes true
    interior-dupes true
    enable-acoustid true
    path-schema "{album_artist}/{album}/{track} - {title}"
    pinned-release "12345678-abcd-1234-efgh-123456789012"
}
```

Paths are relative to `<root>/corpus/`. A source directory applies to all files within its subtree.

### Fields

| Field | Type | Default | Inheritable | Description |
|-------|------|---------|-------------|-------------|
| `library` | string (repeated) | — | yes | Target library names for deployment |
| `can-stash-dupes` | bool | `true` | yes | Whether duplicates from this source can be stashed |
| `interior-dupes` | bool | `true` | yes | Whether intra-source duplicates are flagged |
| `enable-acoustid` | bool | `true` | yes | Whether AcoustID lookups run for this source |
| `path-schema` | string | — | yes | Expected path structure as tag placeholders |
| `pinned-release` | string | — | **no** | MusicBrainz release ID for exclusive assignment |

### Inheritance

Boolean fields use `Option<bool>` — `None` means "inherit from the nearest parent source directory." If no parent sets a value, the system default applies. `pinned-release` is directory-specific and never inherited.

Parent/child relationships are determined by path nesting: `dir "web/releases"` is a parent of `dir "web/releases/bandcamp"`.

## Opinions Reference

All opinions live inside the `opinions { }` block in `config.kdl`. Every field has a sensible default; you only need to specify values you want to change.

```kdl
opinions {
    // top-level opinion fields
    // sub-blocks for grouped settings
}
```

### Top-Level Opinion Fields

| KDL Name | Rust Field | Type | Default | Description |
|----------|-----------|------|---------|-------------|
| `lossy-shit-formats-to-flac` | `lossy_shit_formats_to_flac` | bool | `false` | Capture lossy formats to FLAC containers instead of transcoding to Opus |
| `leave-transactions-open` | `leave_transactions_open` | bool | `false` | Keep one persistent transaction open across modal interactions |
| `watcher-poll-interval-secs` | `watcher_poll_interval_secs` | u64 | `900` | Filesystem watcher polling fallback interval (seconds) |

### `startup` Block

```kdl
startup {
    force-check-all-files false
    vacuum-threshold 0.1
    default-view "health"
}
```

| KDL Name | Type | Default | Description |
|----------|------|---------|-------------|
| `force-check-all-files` | bool | `false` | Verify all indexed files at startup, bypassing mtime optimization |
| `vacuum-threshold` | f64 | `0.1` | Free-page ratio threshold for DB compaction prompt (0.0 disables) |
| `default-view` | string | `"health"` | Landing view after startup: `health`, `search`, `browser`, `inbox`, `external-matches` |

### `quality-resolution` Block

```kdl
quality-resolution {
    inbox-bitrate-fuzz-percent 5.0
}
```

| KDL Name | Type | Default | Description |
|----------|------|---------|-------------|
| `inbox-bitrate-fuzz-percent` | f64 | `5.0` | Bitrate tolerance (%) for treating inbox files as equivalent to corpus |

### `canonicalization` Block

```kdl
canonicalization {
    strip-album-format-suffixes false
}
```

| KDL Name | Type | Default | Description |
|----------|------|---------|-------------|
| `strip-album-format-suffixes` | bool | `false` | Strip EP/LP suffixes during album collision detection |

### `health-detection` Block

```kdl
health-detection {
    required-tags "title" "album" "artist" "album_artist"
    album-artist-only-required-if-compilation true
    single-album-suffix ""
}
```

| KDL Name | Type | Default | Description |
|----------|------|---------|-------------|
| `required-tags` | string list | `"title" "album" "artist" "album_artist"` | Tags that must be present on every track |
| `album-artist-only-required-if-compilation` | bool | `true` | Only require album_artist on compilation albums |
| `single-album-suffix` | string | `""` | Suffix appended to track title when tagging as a single |

### `performance` Block

```kdl
performance {
    worker-threads 16
    db-cache "256mb"
}
```

| KDL Name | Type | Default | Description |
|----------|------|---------|-------------|
| `worker-threads` | int | 2x logical cores | Number of worker threads |
| `db-cache` | size string | `"256mb"` | SQLite page cache per connection. Accepts: plain MB, or suffixed `kb`/`mb`/`gb` |

### `tag-splitting` Block

```kdl
tag-splitting {
    collab "feat" "featuring" "ft" "with" "vs"
    artist ";"
    genre ";"
}
```

| KDL Name | Type | Default | Description |
|----------|------|---------|-------------|
| `collab` | string list | `"feat" "featuring" "ft" "with" "vs"` | Collaboration keywords for artist tag splitting |
| *tag-name* | string list | `";" (ARTIST, GENRE)` | Per-tag separator strings (node name = uppercase tag name) |

### `duplicate-analysis` Block

```kdl
duplicate-analysis {
    fingerprint-similarity-threshold 95.0
    duration-tolerance-ms 2000
    elide-variant-titles true
}
```

| KDL Name | Type | Default | Description |
|----------|------|---------|-------------|
| `fingerprint-similarity-threshold` | f64 | `95.0` | Similarity threshold (0–100) below which pairs aren't duplicates |
| `duration-tolerance-ms` | i64 | `2000` | Duration difference (ms) above which tracks are clustered separately |
| `elide-variant-titles` | bool | `true` | Skip duplicate pairs where titles contain variant keywords (remix, live, etc.) |

### `release-packing` Block

```kdl
release-packing {
    duration-tolerance-pct 0.15
    min-confidence 0.3
    title-preassign-threshold 0.95
    packing-knot-ratio 3.0
    packing-knot-size-limit 50
    singles-before-incompletes true
    allow-resolve-knots-with-discographies true
    low-confidence-max-acoustid-ratio 0.25
    low-confidence-max-album-match 0.30

    candidate-weights {
        acoustid-confidence 0.30
        duration-match 0.30
        title-match 0.10
        artist-match 0.05
        album-match 0.05
        track-number-match 0.20
    }

    elimination-weights {
        acoustid-confidence 0.0
        duration-match 0.25
        title-match 0.30
        artist-match 0.0
        album-match 0.05
        track-number-match 0.40
    }
}
```

| KDL Name | Type | Default | Description |
|----------|------|---------|-------------|
| `duration-tolerance-pct` | f64 | `0.15` | Duration tolerance as fraction (0.0–1.0) for recording match filtering |
| `min-confidence` | f64 | `0.3` | Minimum AcoustID confidence to consider a recording match |
| `title-preassign-threshold` | f64 | `0.95` | Title similarity threshold for pre-assignment (elimination phase 1) |
| `packing-knot-ratio` | f64 | `3.0` | Proposals/inodes ratio for knot extraction (0 disables) |
| `packing-knot-size-limit` | usize | `50` | Max component size before forced knot extraction (0 disables) |
| `singles-before-incompletes` | bool | `true` | Process single-track releases before incomplete packings |
| `allow-resolve-knots-with-discographies` | bool | `true` | Reduce knots using discography releases before greedy resolution |
| `low-confidence-max-acoustid-ratio` | f64 | `0.25` | Max AcoustID ratio for low-confidence downgrade |
| `low-confidence-max-album-match` | f64 | `0.30` | Max album match score for low-confidence downgrade |

**Weight sub-blocks** (`candidate-weights`, `elimination-weights`): Six scoring dimensions, each a f64 weight. See `RELEASE_PACKING_ALGORITHM.md` for scoring details.

### `inbox-organize` Block

```kdl
inbox-organize {
    directory-granularity "leaf"
}
```

| KDL Name | Type | Default | Description |
|----------|------|---------|-------------|
| `directory-granularity` | string | `"leaf"` | `"leaf"` = deepest dirs with audio; `"toplevel"` = direct children of inbox/ |

### `external-matching` Block

```kdl
external-matching {
    acoustid-api-key "your-key-here"
    requests-per-second 3
    mb-requests-per-second 25
    mb-base-url "https://musicbrainz.org/ws/2"
    auto-enrich-on-match true
    mb-cache-ttl-days 30
    preferred-locales "en" "ja"

    credit-routing {
        performer { artist true; title false; composer false }
        vocal { artist true; title true; composer false }
        instrument { artist true; title false; composer false }
        remixer { artist false; title true; composer false }
        feat-format "feat. {artists}"
    }
}
```

| KDL Name | Type | Default | Description |
|----------|------|---------|-------------|
| `acoustid-api-key` | string | `""` (disabled) | AcoustID API key. Empty disables fingerprint matching |
| `requests-per-second` | u32 | `3` | AcoustID rate limit |
| `mb-requests-per-second` | u32 | `25` | MusicBrainz rate limit (raise with a self-hosted mirror) |
| `mb-base-url` | string | `"https://musicbrainz.org/ws/2"` | MusicBrainz API endpoint. Override for local mirrors |
| `auto-enrich-on-match` | bool | `true` | Auto-trigger MB enrichment when AcoustID matches arrive |
| `mb-cache-ttl-days` | u32 | `30` | Days before re-fetching MB cache entries |
| `preferred-locales` | string list | `[]` | BCP 47 locale preference order for artist names |
| `credit-routing` | sub-block | *(see above)* | Per-relation-type routing to tag destinations |
| `feat-format` | string | `"feat. {artists}"` | Format string for vocalist title suffix |

### `disc-extraction` Block

```kdl
disc-extraction {
    disc-tag-name "DISCNUMBER"
    map-letters-to-numbers false
}
```

| KDL Name | Type | Default | Description |
|----------|------|---------|-------------|
| `disc-tag-name` | string | `"DISCNUMBER"` | Tag name for extracted disc identifiers |
| `map-letters-to-numbers` | bool | `false` | Convert letter prefixes to numbers (A→1, B→2, ...) |

### `album-art` Block

```kdl
album-art {
    sidecar-deploy-mode "primary-cover"
}
```

| KDL Name | Type | Default | Description |
|----------|------|---------|-------------|
| `sidecar-deploy-mode` | string | `"primary-cover"` | `"disabled"`, `"primary-cover"`, or `"all"` |

## Environment Variable Overrides

Environment variables override values from `config.kdl`. Config.kdl must still exist — env vars layer on top after file parsing.

**Precedence**: env var > config.kdl > default

### Curated Aliases

Shorthand env vars for the most common settings:

| Env Var | Config Path | Type | Notes |
|---------|------------|------|-------|
| `MM_ROOT` | `root` | path | Archive root directory |
| `MM_ACOUSTID_API_KEY` | `opinions.external_matching.acoustid_api_key` | string | |
| `MM_MB_BASE_URL` | `opinions.external_matching.mb_base_url` | string | |
| `MM_MB_REQUESTS_PER_SECOND` | `opinions.external_matching.mb_requests_per_second` | u32 | |
| `MM_WORKER_THREADS` | `opinions.performance.worker_threads` | usize | |
| `MM_DB_CACHE` | `opinions.performance.db_cache_mb` | size string | `"256mb"`, `"1gb"`, or plain MB |
| `MM_WATCHER_POLL_INTERVAL` | `opinions.watcher_poll_interval_secs` | duration | `"300"`, `"5m"`, `"15min"`, `"1h"` |

### Generic Convention: `MM_CFG__<BLOCK>__<FIELD>`

Any flat scalar opinion field can be overridden using the naming convention:

```
MM_CFG__<BLOCK>__<FIELD>=<value>
```

- `__` (double underscore) separates nesting levels
- Names are SCREAMING_SNAKE_CASE, mapped to kebab-case KDL names (underscores → hyphens)
- Only flat scalar fields (string, bool, int, float) — not sub-blocks or lists
- Unknown paths log a warning and are skipped

**Examples**:

```bash
# Override fingerprint similarity threshold
MM_CFG__DUPLICATE_ANALYSIS__FINGERPRINT_SIMILARITY_THRESHOLD=90.0

# Override vacuum threshold
MM_CFG__STARTUP__VACUUM_THRESHOLD=0.05

# Override MB cache TTL
MM_CFG__EXTERNAL_MATCHING__MB_CACHE_TTL_DAYS=7

# Override sidecar deploy mode
MM_CFG__ALBUM_ART__SIDECAR_DEPLOY_MODE=disabled
```

**Booleans** accept: `true`/`false`, `1`/`0`, `yes`/`no` (case-insensitive).

All applied overrides are logged to `general.log`.

## Validation

`config.validate()` checks:

1. **Archive root exists** — the directory at `root` must exist on disk
2. **Subdirectories exist** — `corpus/`, `libraries/`, `stash/`, `inbox/` must all exist
3. **Same filesystem** — all four subdirectories must reside on the same block device (required for hardlinking)
4. **Source path containment** — no source directory path may escape the corpus directory

Validation runs once at startup. Filesystem layout is assumed stable at runtime.
