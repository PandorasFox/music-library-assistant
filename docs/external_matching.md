# External Metadata Matching

Pulling potential match data from external APIs (AcoustID, and later MusicBrainz, Discogs, etc.) and syncing match data diffs similar to OOB tag syncs. New data source to diff against besides files on disk.

## Current State

Stages 0–3 are implemented and operational. The system is operator-initiated only — external fetches happen when the operator explicitly requests them from the External Matches lateral view.

### Stage 1: AcoustID Lookup Infrastructure — DONE

- `external/acoustid.rs` — HTTP client, POST to AcoustID v2 API, response parsing (`LookupOutcome`: Matches/NoMatch/RateLimited)
- `meta/external/mod.rs` — `ExternalSource` enum (integer-keyed: `AcoustID = 1`), extensible for future sources
- DB tables: `external_matches` (inode, fingerprint, source, recording_id, confidence, raw_response, fetched_at), `external_no_match` (fingerprint, source, queried_at), `external_retry` (inode, fingerprint, source, failed_at, error, retry_count)
- `witch/external_fetch.rs` — autonomous fetch thread with rate limiting, sends results back via channel
- Witch integration: `drain_external_fetch_results()` writes to DB on each tick, `request_external_fetch()` is operator-initiated
- Config: `acoustid_api_key`, `requests_per_second` (default 3), per-dir `enable_acoustid` toggle on `SourceDir`
- "No match" represented as `external_no_match` table entry — absence of a result is cached

### Stage 2: Match Signals & Classification — DONE

- `DeriveExternalMatches` computation: reads `external_matches` table, parses raw AcoustID JSON, compares against corpus tags, classifies, emits signals
- Signal type: `ExternalMatchSignal` with `ExternalMatchData` (recording_id, confidence, classification, diffs, release_id, release_group_id)
- Classification: `MatchClassification` — `ExactMatch` (hidden from UI), `ContentDiff` (tag values differ), `MetadataOnly` (external has tags corpus doesn't)
- Per-tag diffs: `ExternalTagDiff` (tag_name, external_value, corpus_value)
- View types: `ExternalMatchesData` with confidence buckets (`ConfidenceTier`: Perfect/VeryHigh/High/Medium/Low)
- `release_id` and `release_group_id` stored in signal data for future bin-packing

### Stage 3: UI Presentation — DONE

- Lateral view: `ui/external_match_view/` — two-pane layout (65/35), actions section + confidence-bucketed match entries, detail pane with fetch progress (braille bar + counters)
- Review modal: `ui/external_match_modal/` — file list with classification markers (!/?), tag diff display (recording ID, per-tag ext vs corpus values), Accept/Dismiss/Cancel buttons
- Action handler: `ui/action_handlers/external_match.rs` — Accept stages `ApplyTagOpsMutation` via `operator_decisions::stage_decision()` with `ConfirmationGesture`, Dismiss skips entry
- Cache thread: `CacheRequest::WantExternalMatches` / `CacheReady::ExternalMatches`
- Fetch progress: `FetchProgress` struct with live counters (total/processed/matched/no_match/retries), piped from fetch thread → Witch → UI each frame

### Stage 0: Path-Tag Schema (dir-config extension) — DONE

- `config/path_schema.rs` — Parser for template DSL (`$TAG`, `${TAG}`, `$[optional $TAG]`), `PathTagSchema` type with `extract()` matcher
- `SourceDir` extended with `path_schema: Option<PathTagSchema>`, parsed from `path-schema` field in `dirs.kdl`
- `meta/computations/analysis/path_schema.rs` — `DetectPathTagMismatches` computation, compares path-extracted metadata against corpus tags
- Signal type: `PathTagMismatchSignal` with `PathTagMismatchData` (source_dir, schema_template, mismatch_kind)

## Remaining Work

### Dismiss/Ignore Persistence — NOT STARTED

Currently "Dismiss" in the review modal skips the entry for the current session but doesn't persist. Dismissed matches resurface on next derivation.

**Scope:**
- New DB table for ignored match UUIDs (recording_id + inode pairs) so dismissed matches don't resurface
- Integration with `DeriveExternalMatches` to filter out ignored matches before signal emission
- UI affordance to view/clear ignored matches

## MusicBrainz Tag Names

MM writes three MusicBrainz entity IDs as Vorbis Comment tags when a match is approved:

| Tag (default) | MB Entity | Written When |
|--------------|-----------|-------------|
| `MUSICBRAINZ_RECORDING` | Recording MBID | Always (identifies the abstract audio work) |
| `MUSICBRAINZ_RELEASE` | Release MBID | Always (identifies the album/single/EP) |
| `MUSICBRAINZ_TRACK` | Track MBID | When available in MB data (identifies the track's slot on a release) |

Tag names are configurable via `mb-tag-names` in the `external-matching` config block. See `docs/CONFIGURATION.md` for the full reference and Picard-compatible presets.

**Health check elision**: Files with both `track` and `release` MB tags present are considered "fully MusicBrainz-tagged" and are automatically excluded from tag-based health checks (missing tags, canonicity, compound tags, inconsistent album artist, disc extraction, path-tag mismatches). A `MusicBrainzTagged` signal is emitted for these files, providing a count of externally-authoritative vs locally-comprehended corpus files.

### Stage 4 (future): Multi-Source, Release Bin-Packing & Discogs

- Extend to MusicBrainz (tier-2 lookups from recording IDs → release metadata) and Discogs
- Release bin-packing: group inodes by release ID for batch review (groundwork exists — `release_id`/`release_group_id` already stored in signal data)
- Handle conflicting matches across sources, preference ordering
- `ExternalSource` enum already has integer-keyed discriminant, `external_matches.source` column ready
