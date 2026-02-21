# External Metadata Matching — Staged Roadmap

Background pulling of potential match data from external APIs (MusicBrainz, Discogs, etc.) and syncing match data diffs similar to OOB tag syncs. New data source to diff against besides files on disk.

## Stage 0: Path-Tag Schema (dir-config extension)

**Why first:** Cross-cutting — benefits external matching *and* existing signals (path correction, tag correction). Extends `SourceDir` in `dirs.kdl` with a new concept that everything else can build on.

**Scope:**
- Extend `SourceDir` (in `config/types.rs`, `config/dirs.rs`) with an optional path-tag schema field — a pattern like `/$LABEL/$CATALOGNUMBER/$ARTIST - $ALBUM - $TITLE.flac` that maps path segments to tag semantics
- Parser for the schema DSL (probably its own small module)
- Extraction function: given a corpus path + schema → `HashMap<TagKey, String>` of inferred metadata
- New signal type: `PathTagMismatch` — when extracted path metadata disagrees with actual tags (usable immediately by existing analysis computations)
- Standalone feature with immediate value

**Key decisions for detailed planning:**
- DSL syntax for the schema patterns (glob-style vs template-style vs regex)
- How to handle optional/variable segments
- Whether schemas can be inherited (parent dir schema applies to children)

## Stage 1: AcoustID Lookup Infrastructure

**Scope:** Get fingerprints matched against AcoustID's API, store raw results, no UI yet.

**Architecture (following Witch patterns):**
- New `ExternalLookup` computation type — runs in analysis phase, queries which inodes have fingerprints but no cached AcoustID result
- Background HTTP client with rate limiting (AcoustID allows ~3 req/sec). I/O-bound work that fits naturally into rayon worker threads — the Witch spawns these, same as any computation
- New DB table: `external_matches` — stores raw AcoustID responses keyed by fingerprint, with timestamps for staleness/retry logic
- Dir-config extension: `external_matching: bool` on `SourceDir` (default true), so operators can opt directories out
- Operator-level config: API keys for AcoustID (and later MusicBrainz/Discogs) in the main config

**Key decisions for detailed planning:**
- Table schema for `external_matches` — what granularity? Per-fingerprint? Per-inode?
- Staleness policy — how often to re-query? Never re-query? Only on operator demand?
- Error handling — rate limit hits, network failures, partial results
- How to represent "no match found" (important — absence of a result is itself information)
- Whether AcoustID lookup is a computation or a new task category (it's I/O-bound, not CPU-bound like most computations — but the Witch already handles I/O-bound indexing work fine)

## Stage 2: Match Signals & Release Bin-Packing

**Scope:** Process raw AcoustID results into typed signals. Classify matches. Group into release candidates.

**Architecture:**
- New derivation/analysis computation: `DeriveExternalMatches` — reads `external_matches` table, compares against existing tags
- Match classification enum (new type in `meta/signals/data.rs`):
  - `ExactMatch` — all tags agree
  - `SimilarTags` — case/punctuation/whitespace differences only
  - `ContentDiff` — substantive tag value differences (like OOB tag sync)
  - `NoMatch` — fingerprint submitted but AcoustID returned nothing
- New signal: `ExternalMatchSignal` — per-inode, carrying the classification + diff data (modeled after `OutOfBandTagSyncSignal` with `TagMismatchEntry` style diffs)
- Release bin-packing: group inodes by MusicBrainz release ID (AcoustID returns recording IDs → release group lookups). This is where MusicBrainz API comes in as a second-tier lookup
- Ignore/dismiss mechanism: `IgnoredExternalMatch` table or signal suppression (similar pattern to how `can_stash_dupes` suppresses duplicate signals)
- Path-tag schema (from Stage 0) feeds into match quality — extracted path metadata supplements tag comparison

**Key decisions for detailed planning:**
- Signal granularity — per-inode or per-release-group?
- How bin-packing interacts with existing `FingerprintOverlapSignal` (internal duplicates vs external matches are different concerns)
- MusicBrainz rate limits (1 req/sec with user-agent) and whether to fold this into Stage 1's infrastructure or keep separate
- Whether "ignore" is per-match, per-inode, or per-release

## Stage 3: UI Presentation

**Scope:** New lateral view tab, progress visibility, match review interface.

**Architecture (following existing patterns):**
- New `LateralView::ExternalMatches` variant in the titlebar ring
- New `ActiveView::ExternalMatches(ExternalMatchesState)` with the standard `handle_key → Action → dispatch` pattern
- View layout: probably `ThreePaneLayout` — left pane for match groups/releases, right pane for per-track diffs (similar to how OOB tag sync shows mismatches)
- Progress indicator options:
  - **Option A:** Badge/counter on the ExternalMatches tab (like `deploy_needs_action` highlighting)
  - **Option B:** Dedicated status line showing fetch progress (e.g., "AcoustID: 142/500 queried")
  - **Option C:** Both — badge for "matches need review", status line for fetch progress
- Match review actions: Accept (apply external tags → mutation), Ignore (suppress signal), Skip
- Cache thread integration: `CacheRequest::WantExternalMatches` for async loading of match data
- Resolution mutations: new `ApplyExternalTagsMutation` — similar to `AssimilateDiskTagsToDb` but source is external API data instead of disk tags

**Key decisions for detailed planning:**
- Tab placement in the ring
- Whether match review uses the existing transaction/decision system or has its own flow
- How to show release-level grouping (a release might span tracks across different corpus directories)
- Whether "accept" applies all tags from the match or lets the operator cherry-pick per-tag

## Stage 4 (future): Multi-Source & Discogs

**Scope:** Extend to Discogs, handle conflicting matches across sources, preference ordering.

Far enough out that it doesn't need detailed planning yet, but Stage 1's `external_matches` schema should be designed with a `source` discriminant column from the start.

## Dependencies

```
Stage 0 (Path-Tag Schema)  ──────────────────────────────┐
    standalone, immediate value                           │
                                                          ▼
Stage 1 (AcoustID Infrastructure)  ──→  Stage 2 (Signals & Bin-Packing)  ──→  Stage 3 (UI)
    DB + API + config                    classification + grouping              presentation + review
```

Stages 0 and 1 can proceed in parallel. Stage 2 depends on Stage 1's DB schema being settled. Stage 3 depends on Stage 2's signal types being defined.
