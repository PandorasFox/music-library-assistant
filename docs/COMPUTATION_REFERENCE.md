# Computation Reference

> **Maintenance Requirement**: Any changes to computation logic, signal emission, or spawn
> behavior MUST be reflected in this document. Update the tables before or alongside code changes.

## Source Location

All computation code lives in `src/meta/computations/`:
- **Enum & dispatch**: `mod.rs` (Computation enum, `execute_single()`)
- **Witness**: `types.rs` (ComputationWitness sealed module)
- **Helpers**: `helpers.rs` (signal emission/clearing helpers)
- **Stats**: `stats.rs` (thread-local stats + read-only DB connections)
- **Observation phase**: `observation/mod.rs`, `observation/executors.rs`
- **Derivation phase**: `derivation/mod.rs`, `derivation/executors.rs`
- **Analysis phase**: `analysis/mod.rs`, `analysis/schedule.rs`, `analysis/duplicates.rs`, `analysis/tags.rs`, `analysis/deploy.rs`, `analysis/formats.rs`, `analysis/album_art.rs`, `analysis/album_art_info.rs`, `analysis/image_files.rs`, `analysis/external_matches.rs`, `analysis/release_packing.rs` (4-stage pipeline)
- **Pipeline infrastructure**: `mod.rs` (`PipelineStage` enum, `deferred_phases` on `ComputationResult`)

## Phase Overview

MM uses three-phase computations with compile-time enforced boundaries:

| Phase | Purpose | Triggers |
|-------|---------|----------|
| **Observation** | Pure corpus filesystem observation without inference | Startup, periodic rescan |
| **Derivation** | First-level derivations comparing observations to index | After Observation completes |
| **Analysis** | Full-corpus analysis requiring complete awareness | After Derivation completes |

**Phase Boundary Enforcement**: Each phase has its own `Result` struct with a `spawn: Vec<PhaseComputation>` field that only accepts that phase's computations. Attempting to spawn a computation from a different phase will result in a compile error.

### Staged Pipelines (Barrier-Separated Phases)

Some computations require multi-stage execution with barriers between stages. The `deferred_phases` mechanism on `ComputationResult` enables this:

1. **Stage 1 (orchestrator)** returns `spawn: [N computations]` (queued immediately) plus `deferred_phases: [(Resolve, [...]), (Analyze, [...])]`
2. The Witch's `tick()` queues spawned computations and stores `deferred_phases` in `pending_computation_phases`
3. All Stage 2 computations run in parallel. When all complete and the db write queue drains, `transition_to_completed()` fires
4. `transition_to_completed()` pops the next phase from `pending_computation_phases`, waits for the db write queue to drain, then queues that phase's computations
5. Repeats until all phases are drained, then transitions to Done

This follows the same pattern as `pending_mutation_phases` (used for two-stage derivation). Pipeline stages are labeled via `PipelineStage` enum (`Resolve`, `Analyze`, `DependentAnalysis`) for logging.

---

## Computations by Phase

### Observation Phase

| Computation | Description |
|-------------|-------------|
| WalkCorpus | Enumerate directories, spawn per-directory scans. Has `force_check` parameter. |
| ScanCorpusDirectory | Collect disk state, emit FileInCorpus signals, return observed inodes to Witch. Has `force_check` parameter. |
| VerifyTags | Verify disk tags match indexed tags, emit classification signals. Pending-write aware. |

### Derivation Phase

| Computation | Description |
|-------------|-------------|
| ScheduleSecondLevelDerivations | Orchestrator for per-directory derivation work |
| DeriveDirectorySignals | Derive UnindexedFile, MissingFile, HealthyFile signals |
| UpdateCorpusFileSignals | Lightweight per-file signal update (post-mutation) |
| UpdateLibraryFileSignals | Library-side signal updates |
| UpdateDeploySignals | Update deployment status signals |
| StashAndReplaceSidecars | Stash inferior sidecar images, write CAA replacements, clean up DB/signals |
| WalkLibrary | Enumerate library directories, spawn per-directory scans |
| ScanLibraryDirectory | Scan library directory, return observed files to Witch |
| ReconcileLibraryFiles | Reconcile observed library files against DB (set reconciliation) |
| WatcherUpsertLibraryFile | Apply a single library FileCreated/FileChanged event to the DB (per-event watcher sync) |
| WatcherDeleteLibraryFile | Apply a single library FileRemoved event to the DB (per-event watcher sync) |

### Analysis Phase

| Computation | Description |
|-------------|-------------|
| ScheduleContentAnalysis | Orchestrator: spawns all detection computations |
| DetectFingerprintOverlaps | Find tracks with identical fingerprints |
| DetectDuplicateInodes | Find tracks sharing same inode |
| DetectMusicBrainzTagged | Detect files with fully-applied MusicBrainz release tags (both configured track + release tags present). Emits MusicBrainzTagged per-file signals. Reconcile-based: clears when tags removed |
| DetectMissingTags | Find tracks missing required tags. Routes ALBUM-only-missing (with ARTIST+TITLE) to MissingAlbumSingleSignal; checks ExpectedMissingTag for suppression. Skips MusicBrainz-tagged files |
| DetectMetadataDuplicates | Find tracks with identical tag sets |
| DetectArtistTagCanonicalizations | Find `artist` tag canonicalization opportunities. Reconciles only `TagCanonicitySignal` keys prefixed `artist:` |
| DetectAlbumArtistTagCanonicalizations | Find `albumartist` tag canonicalization opportunities (across separator variants). Reconciles only keys prefixed `album_artist:` |
| DetectAlbumTagCanonicalizations | Find `album` tag canonicalization opportunities (gated by disjoint release-id checks). Reconciles only keys prefixed `album:` |
| DetectGenreTagCanonicalizations | Find `genre` tag canonicalization opportunities. Reconciles only keys prefixed `genre:` |
| DetectInconsistentAlbumArtist | Find inconsistent album_artist across albums. Skips groups where any track has `COMPILATION=0` |
| DetectCompoundTagValues | Orchestrator: spawns DetectCompoundTagsForInode for each dirty corpus inode. Parallelizes detection across worker threads. |
| DetectCompoundTagsForInode | Per-inode: walks the priority-ordered `SplitRule` chain from `TagSplittingOpinions` (separator and collaboration keyword rules). First matching rule wins per tag value. Emits per-file CompoundTag signals. Skips CanonicalTag whitelisted values. |
| DetectLosslessRemux | Find files with lossless non-Vorbis containers (WAV, AIFF, APE, WV). Uses dirty inode tracking — only checks recently (re)indexed inodes |
| DetectPathTagMismatches | Detect files whose paths don't match their source dir's path-tag schema. Extracts tag values from path structure, compares against DB tags (case-insensitive). Emits per-file PathTagMismatch signals |
| DeriveExternalMatches | Derive external match signals from AcoustID results. Compares recording metadata (title, artist, album) against corpus tags using raw string equality. Emits per-file ExternalMatch signals with classification and diffs |
| PackReleases { incremental } | **Stage 1 orchestrator** for the release bin-packing pipeline. Loads AcoustID external matches, filters by min_confidence, identifies candidate releases, loads tracklists and locale-resolved artist names. Uses ALL recordings per inode (not just the best-confidence one). Candidates dedup per (release_id, inode) keeping highest confidence. Writes `dir_file_count` to the candidates table. **Pinned releases**: adds pinned release IDs from `dirs.kdl`; injects synthetic candidate rows (confidence=1.0, empty recording_id) for inodes in pinned dirs without an AcoustID candidate for the pinned release. **Modes:** `incremental=false` (operator-initiated full repack) runs `truncate_packing_tables`, writes the full manifest, writes all candidates, spawns N `ScoreReleaseCandidates`, then defers `ComputeReleaseMappings` (Stage 3) — which chains through Stage 4. `incremental=true` (live-ingestion auto-trigger after AcoustID fetch) reads `release_packing` dirty inodes (set by TAGS-scope mutations via `PER_INODE_TAG_SCOPE_COMPUTATIONS`), computes `affected_releases = (releases reachable from those dirty inodes' recordings) ∪ (pinned releases)`, runs `delete_packing_data_for_release` per affected release (instead of truncating), writes manifest/candidates only for affected releases, filters `deduped` candidates to `release_id ∈ affected_releases`, clears the dirty-inode flags, spawns N `ScoreReleaseCandidates`, and **does not defer mapping/MIS** — existing PackedRelease and ReleasePacking signals stay intact for the next full repack to reconcile against the updated scoring data. Threads `incremental` through to Stage 4 for unmatched-detection inode-exclusion handling. |
| ScoreReleaseCandidates | **Stage 2** (N parallel). Per-release directory-constrained candidate scoring and elimination matching. Loads release tracklist, finds corpus inodes matched via AcoustID recordings linked to this release. **Pinned releases**: if this release is pinned by one or more source dirs, `dir_candidate_inodes` is restricted to those dirs only before directory selection runs; multi-medium pinned releases coalesce via standard `PerMedium` sibling-dir mapping. **Directory selection** constrains all packing to a single target directory (or sibling directories for multi-medium releases). Single-medium: prefers directory where file count matches track count with the most AcoustID candidates, tiebreak on score sum. Multi-medium: tries sibling directory bijection (each dir's file count matches a medium's track count, same parent), falls back to single-dir. **Candidate scoring** uses configurable `candidate_weights` with 6 independent dimensions (default: AcoustID confidence 0.30, duration match 0.30, title match 0.10, artist match 0.05, album match 0.05, track number match 0.20). Optimal per-release assignment via Hungarian algorithm (Kuhn-Munkres) on directory-filtered candidates only. After AcoustID scoring, performs per-release **elimination matching** for unfilled slots within the same target directory using configurable `elimination_weights` with the same 6 dimensions (default: duration match 0.25, title match 0.30, artist match 0.0, album match 0.05, track number match 0.40; no AcoustID). Elimination uses two phases: title pre-assignment (threshold 0.95, 1:1 unambiguous matches only) followed by Hungarian on remaining pairs (no score threshold — directory constraint provides the quality gate). Artist match defaults to 0 in elimination because rip artist tags often diverge from MB release-level credits (e.g., individual composers vs game studio). Both weight sets are configurable under `release-packing` in config.kdl. Writes all candidates + optimal flags to `release_packing_scores` table with `match_method` column (`AcoustId` or `Elimination`). See `docs/RELEASE_PACKING_ALGORITHM.md` for full algorithm reference |
| ComputeReleaseMappings | **Stage 3a** (after barrier). Orchestrator for tiered MIS resolution. Loads optimal proposals from `release_packing_scores` and directory metadata from `release_packing_candidates`. Classifies each proposal into a quality tier: **Perfect** (all slots filled, 1:1 directory↔release mapping, no leftover files; multi-medium aware), **FullMatch** (all slots filled but cross-directory or extras), **Incomplete** (some but not all slots filled), **Single** (single-track release). With directory-constrained packing, cross-directory scattering is structurally impossible. **Pinned pre-acceptance**: before MIS rounds, detects conflicted pinned releases (more pinning dirs than media) and emits `PinnedReleaseConflict` for them — skipping them entirely. Non-conflicted pinned proposals are pre-accepted: signals emitted immediately, inodes claimed. Non-pinned proposals overlapping claimed inodes are rejected in subsequent MIS rounds. Packages state and defers `MapPerfectReleases` |
| MapPerfectReleases | **Stage 3b** orchestrator (after barrier). Filters to proposals with entirely unclaimed inodes, finds connected components, emits isolated nodes (size-1 components) directly, spawns N `ResolvePackingComponent` for multi-node components. Defers `MapFullMatchReleases` |
| MapFullMatchReleases | **Stage 3c** orchestrator (after barrier). Culls tainted proposals, deduplicates by inode signature, extracts knots (greedy best-scorer, emits `PackingKnot` signals directly), spawns N `ResolvePackingComponent` for clean components. Defers dynamically: `MapIncompleteReleases` (default) or `MapSingleReleases` (when `singles-before-incompletes` is true) |
| MapIncompleteReleases | **Stage 3d** orchestrator (after barrier). Same cull/dedup/knot/component pipeline as FullMatch. Spawns N `ResolvePackingComponent`. Defers dynamically to the other of Singles/EmitUnmatchedSignals |
| MapSingleReleases | **Stage 3e** orchestrator (after barrier). Same pipeline as FullMatch/Incomplete. Each single proposal has a 1-element inode set, so MIS naturally picks best-scoring per inode. Defers dynamically to the other of Incomplete/EmitUnmatchedSignals |
| ResolvePackingComponent | Per-component MIS solver (spawned in parallel by tier orchestrators). Carries its own `Vec<Proposal>` and tier. Builds local conflict graph, solves via bitmask (≤25) or BnB (>25), computes local alternatives_count, emits `PackedRelease` + `ReleasePacking` signals + pending AcoustID submissions. No spawn, no deferred phases |
| EmitUnmatchedSignals { incremental } | **Stage 4** (after barrier). Post-resolution gap analysis. Identifies unmatched corpus tracks (both from candidate pipeline and fingerprinted files with no AcoustID match) → `UnmatchedCorpusTrack` signals. **Incremental mode:** excludes MB-tagged inodes from unmatched detection to prevent false positives for solved releases that were skipped in Stage 1. Computes per-release filled slots from actual inode→release assignments (not optimal scores) → `UnfilledReleaseSlot` signals. **Coverage filtering:** suppresses `UnfilledReleaseSlot` signals for releases where every candidate inode is already assigned to a full-match release (fully-covered releases represent cached MB entries, not real gaps) |
| DetectDiscExtractions | Detect extractable disc numbers from ALBUM and TRACKNUMBER tags. Pass 1 (album): scans ALBUM tags for `,?\s*disc\s+(\d+)\s*$` pattern. Pass 2 (track number): scans TRACKNUMBER for `^([A-Za-z]+)(\d+)$`, groups by release context (album+album_artist), only emits if group has ≥2 files. Emits DiscExtraction aggregate signals |
| AnalyzeFingerprintOverlaps | Analyze fingerprint overlaps for similarity, variants, quality tier partitioning |
| DetectCrossSourceOverlaps | Cluster FingerprintOverlap signals by source directory (from config `dir` stanzas). Within-source overlaps ignored. |
| IndexImageFile | Index corpus image file metadata (format, dimensions, role) into image_info table. Dirty-inode computation |
| DetectDeployConflicts | Detect path collisions in deployment |
| DetectReleaseOverlaps | Detect cross-source album-directory-level release overlaps |
| DeriveDeployHealthSignals | Derive library health signals (per library) |
| DeriveCorpusDeployStatus | Derive corpus deploy status |

---

## Computation Signal Matrix

### Observation Phase Computations

| Computation | Spawns | Signals Emitted | Signals Cleared |
|-------------|--------|-----------------|-----------------|
| WalkCorpus | ScanCorpusDirectory × N (propagates `force_check`) | — | — |
| ScanCorpusDirectory | VerifyTags + VerifyAudio (all indexed, if `force_check=true`) | FileInCorpus | — | Also indexes directory entry (is_dir=1) in files table with read guard (skips write if entry already matches by zone+inode+mtime). Returns observed inodes to the Witch via Result (accumulated in tick(), consumed by queue_derivation_computations()). |
| VerifyTags | — | OutOfBandTagConflict, OutOfBandTagSync, MtimeOnlyMismatch, CorruptFile | OutOfBandTagConflict, OutOfBandTagSync, MtimeOnlyMismatch (mutual exclusion) | Pending-write aware: when `pending_write` marker exists and tags are clean, skips mtime check and clears all OOB signals. Always updates `files.mtime` from disk. |
| VerifyAudio | — | CorruptFile | CorruptFile (if audio valid) |

### Derivation Phase Computations

| Computation | Spawns | Signals Emitted | Signals Cleared |
|-------------|--------|-----------------|-----------------|
| ScheduleSecondLevelDerivations | WalkLibrary × N | MissingDirectory | MissingDirectory (if dir exists again) |
| DeriveCorpusSignals | — | UnindexedFile, MissingFile, HealthyFile, MovedFile | UnindexedFile, MissingFile, HealthyFile (stale); skips HealthyFile for OOB-flagged files. **Move detection**: same-zone (disk path ≠ indexed path in `both` set). **GC backstop**: clears orphaned signals for inodes not in disk ∪ index. Receives observed inodes from the Witch (accumulated from FS watcher). |
| UpdateCorpusFileSignals | — | FileInCorpus, UnindexedFile, MissingFile, HealthyFile | FileInCorpus, UnindexedFile, MissingFile, HealthyFile |
| UpdateLibraryFileSignals | — | — | LibraryLeftover, LibraryStale |
| UpdateDeploySignals | — | DeployedHealthy | DeployReady, LibraryLeftover, LibraryStale |
| StashAndReplaceSidecars | — | — | All corpus signals for stashed inodes | Moves old sidecar to cover-art stash, writes new bytes, drops old inode from files index. Queued by Witch when CAA fetch scheduler reports sidecar replacements. |
| WalkLibrary | ScanLibraryDirectory × N | — | — |
| ScanLibraryDirectory | — | — | — | Returns observed library files to the Witch via Result (accumulated in tick()). No direct DB writes. |
| ReconcileLibraryFiles | — | — | — | Set reconciliation: compares observed files against DB. Upserts new/changed files, deletes stale files, skips unchanged. Queued at startup awakening and on every steady-state derivation tick as a backstop for crash recovery. |
| WatcherUpsertLibraryFile | — | — | — | Per-event upsert of one library file into `inodes`+`inode_paths`. Queued by the Witch's watcher event handler on `FileCreated`/`FileChanged` for the library zone. Drives steady-state DB sync without waiting for the next `ReconcileLibraryFiles` tick. |
| WatcherDeleteLibraryFile | — | — | — | Per-event removal of one library file from `inode_paths` (and `inodes` if no other paths remain). Queued by the Witch's watcher event handler on `FileRemoved` for the library zone. |

### Analysis Phase Computations

| Computation | Spawns | Signals Emitted | Signals Cleared |
|-------------|--------|-----------------|-----------------|
| ScheduleContentAnalysis | All detection computations (except fingerprint-dependent: AnalyzeFingerprintOverlaps and DetectCrossSourceOverlaps are deferred phases of DetectFingerprintOverlaps) | — | — |
| DetectMusicBrainzTagged | — | MusicBrainzTagged | MusicBrainzTagged (via hash-based corpus reconciliation). Queries corpus_tags for inodes with both configured MB track + release tags present. Tag-based health computations (DetectMissingTags, Detect{Artist,AlbumArtist,Album,Genre}TagCanonicalizations, DetectCompoundTagValues, DetectInconsistentAlbumArtist, DetectDiscExtractions, DetectPathTagMismatches) load this inode set and skip MB-tagged files |
| DetectFingerprintOverlaps | Defers AnalyzeFingerprintOverlaps + DetectCrossSourceOverlaps (DependentAnalysis pipeline barrier) | FingerprintOverlap | FingerprintOverlap (stale) |
| DetectDuplicateInodes | — | DuplicateInode | DuplicateInode (stale) |
| DetectMissingTags | — | MissingTag, MissingAlbumSingleSignal | MissingTag (via hash-based reconciliation), MissingAlbumSingleSignal (via hash-based reconciliation). Files with ALBUM missing but ARTIST+TITLE present are routed to MissingAlbumSingleSignal (keyed by lowercased artist) instead of MissingTag. Checks ExpectedMissingTag to suppress known-acceptable missing-album inodes. Skips MusicBrainz-tagged files |
| DetectMetadataDuplicates | — | MetadataDuplicate | MetadataDuplicate (via hash-based reconciliation) |
| DetectArtistTagCanonicalizations | — | TagCanonicity (`artist:*` keys) | TagCanonicity (via hash-based reconciliation, **scoped** to `artist:*` keys). Skips collision groups where any variant has a CanonicalTag signal. One of four parallel sibling computations split from the original monolithic detector — runs in parallel via the rayon pool with album/album-artist/genre |
| DetectAlbumArtistTagCanonicalizations | — | TagCanonicity (`album_artist:*` keys) | TagCanonicity (via hash-based reconciliation, **scoped** to `album_artist:*` keys). Merges across compound-tag separator variants (ALBUMARTIST, ALBUM_ARTIST, ALBUM ARTIST) |
| DetectAlbumTagCanonicalizations | — | TagCanonicity (`album:*` keys) | TagCanonicity (via hash-based reconciliation, **scoped** to `album:*` keys). Loads `strip_album_format_suffixes` from config. Filters out groups whose variants have disjoint release ids (ISRCs / catalog numbers / MB release IDs / years) |
| DetectGenreTagCanonicalizations | — | TagCanonicity (`genre:*` keys) | TagCanonicity (via hash-based reconciliation, **scoped** to `genre:*` keys) |
| DetectCompoundTagValues | DetectCompoundTagsForInode (per inode) | — | CompoundTag (all, before spawning) |
| DetectCompoundTagsForInode | — | CompoundTag (per-file) | — |
| DetectInconsistentAlbumArtist | — | InconsistentAlbumArtist | InconsistentAlbumArtist (via hash-based reconciliation). Groups with `COMPILATION=0` on any track are suppressed (no signal emitted) |
| DetectLosslessRemux | — | LosslessRemux | LosslessRemux (via dirty inode tracking — only processes recently indexed inodes) |
| AnalyzeFingerprintOverlaps | — | SubparDuplicate, RedundantDuplicate | SubparDuplicate (via hash-based corpus reconciliation), RedundantDuplicate (via hash-based aggregate reconciliation). Uses enum-based equivalence-class partitioning (QualityTier = FormatClass + metric). Best tier with >1 file → RedundantDuplicate; lower tiers → SubparDuplicate with reason (SubparFormat, SubparBitrate, SubparSampleRate). Re-release elision: album checked before ISRC when catalog numbers absent. Skips fingerprint groups with an ExpectedDuplicate signal (operator whitelist). Skips groups entirely within a single source dir that has `interior-dupes false` in config |
| DetectPathTagMismatches | — | PathTagMismatch | PathTagMismatch (via hash-based corpus reconciliation). For each corpus audio file with a matching source dir schema (direct or inherited), strips source dir prefix and extension, runs schema extraction, compares extracted tags against DB tags (case-insensitive). Emits StructureMismatch or ValueMismatch signals |
| DetectDiscExtractions | — | DiscExtraction | DiscExtraction (via hash-based aggregate reconciliation). Pass 1: scans ALBUM tags from corpus_tags for `,?\s*disc\s+(\d+)\s*$` pattern. Pass 2: scans TRACKNUMBER tags for `^([A-Za-z]+)(\d+)$` prefix pattern, groups by release context (album+album_artist), only emits if group has ≥2 files |
| DeriveExternalMatches | — | ExternalMatch | ExternalMatch (via hash-based corpus reconciliation). For each corpus inode with AcoustID matches, parses stored raw response JSON, compares recording title/artist/album against corpus tags (raw string equality), classifies as ExactMatch/ContentDiff/MetadataOnly. Triggered by EXTERNAL, TAGS, or FILES scope |
| PackReleases | ScoreReleaseCandidates × N; defers ComputeReleaseMappings (which chains remaining stages) | — | — | Stage 1 orchestrator. Loads AcoustID matches, uses ALL recordings per inode (deduped per (release_id, inode) keeping highest-confidence), writes manifest to `release_packing_manifest` (includes `media_count` column), writes `dir_file_count` to candidates table, truncates intermediate tables, spawns per-release scorers. **Pinned releases**: adds pinned release IDs from `dirs.kdl` `pinned_release` fields to `all_release_ids` so their tracklists are fetched; injects synthetic candidate rows (confidence=1.0, empty recording_id) for inodes in pinned dirs that lack an AcoustID candidate for the pinned release. Clears stale ReleasePacking signals if no matches. Triggered manually (operator), reactively (incremental, after AcoustID fetch), or by the Witch's idle-timer promotion (full, after accumulated incremental passes — config: `release-packing { idle-full-repack-after-secs }`, default 30 min) |
| ScoreReleaseCandidates | — | — | — | Stage 2. Per-release directory-constrained scoring + elimination matching. 6 independent scoring dimensions: acoustid_confidence, duration_match, title_match, artist_match, album_match, track_number_match. **Pinned releases**: if this release is pinned by one or more source dirs, `dir_candidate_inodes` is restricted to those dirs only before directory selection runs — all downstream scoring proceeds normally within the pinned dir set. Multi-medium pinned releases coalesce via standard `PerMedium` sibling-dir mapping. Directory selection constrains all packing to a single target directory (or sibling directories for multi-medium). Optimal assignment via Hungarian algorithm on directory-filtered candidates. Then per-release elimination within the same target directory(ies): title pre-assignment (0.95 threshold, 1:1 only) followed by Hungarian on remaining pairs (no score threshold). Per-medium affinity enforced for multi-medium releases. Writes all candidates + optimal picks to `release_packing_scores` table with `match_method` column (AcoustId or Elimination). No signals emitted (intermediate data only) |
| ComputeReleaseMappings | Defers MapPerfectReleases | PackedRelease, ReleasePacking, AlternativeReleasePacking, VariousArtistsOverride (pinned pre-accepts), PinnedReleaseConflict | Bulk-clears all packing signal tables at start | Stage 3a orchestrator. Classifies proposals into quality tiers (Perfect, FullMatch, Incomplete, Single), packages state for MIS rounds. **Pinned pre-acceptance**: before MIS rounds, detects pinned release conflicts (more pinning dirs than media) and emits `PinnedReleaseConflict` for those — skipping them entirely. Non-conflicted pinned proposals are pre-accepted: their signals are emitted immediately and their inodes are claimed. Non-pinned proposals overlapping claimed inodes are rejected in subsequent MIS rounds |
| MapPerfectReleases | Spawns N ResolvePackingComponent, defers MapFullMatchReleases | PackedRelease, ReleasePacking, AlternativeReleasePacking, VariousArtistsOverride (isolated nodes only) | Stage 3b orchestrator. Builds inode-signature groups for alternative detection (bookkeeping only, no filtering). Emits isolated nodes directly with siblings, spawns parallel solvers with signature_siblings in ComponentData |
| MapFullMatchReleases | Spawns N ResolvePackingComponent, defers Incomplete or Singles (config-dependent) | PackedRelease, ReleasePacking, PackingKnot, AlternativeReleasePacking, VariousArtistsOverride (isolated + knots) | Stage 3c orchestrator. Dedup captures siblings as AlternativeRelease metadata. Cull/dedup/knot-extract, spawns parallel solvers with signature_siblings |
| MapIncompleteReleases | Spawns N ResolvePackingComponent, defers Singles or EmitUnmatchedSignals (config-dependent) | PackedRelease, ReleasePacking, PackingKnot, AlternativeReleasePacking, VariousArtistsOverride (isolated + knots) | Stage 3d orchestrator. Same pipeline as FullMatch |
| MapSingleReleases | Spawns N ResolvePackingComponent, defers Incomplete or EmitUnmatchedSignals (config-dependent) | PackedRelease, ReleasePacking, PackingKnot, AlternativeReleasePacking, VariousArtistsOverride (isolated + knots) | Stage 3e orchestrator. Same pipeline as FullMatch |
| ResolvePackingComponent | — | PackedRelease, ReleasePacking, AlternativeReleasePacking, VariousArtistsOverride | Per-component MIS solver. Self-contained: builds conflict graph, solves, emits signals. Emits AlternativeReleasePacking for each winner's signature siblings. Emits VariousArtistsOverride via two-tier lookup (exact alternatives first, competing proposals fallback). Records pending AcoustID submissions for elimination winners |
| EmitUnmatchedSignals { incremental } | — | UnmatchedCorpusTrack, UnfilledReleaseSlot | UnmatchedCorpusTrack (via hash-based corpus reconciliation), UnfilledReleaseSlot (via hash-based aggregate reconciliation). Stage 4. Emits UnmatchedCorpusTrack for inodes that were scored but not assigned AND fingerprinted corpus files with no AcoustID match. **Incremental mode:** excludes MB-tagged inodes from unmatched detection to prevent false positives for solved releases skipped in Stage 1. Builds filled_slots from actual inode→release assignments (not optimal scores). Coverage filtering: suppresses UnfilledReleaseSlot signals for releases where every candidate inode is already assigned to a full-match release (fully-covered releases are cached MB entries, not real gaps). Only emits UnfilledReleaseSlot for releases with at least one filled slot |
| DetectCrossSourceOverlaps | — | CrossSourceOverlap (keyed by sorted source pair, e.g., "bandcamp\|indie") | CrossSourceOverlap (via hash-based aggregate reconciliation). Skips source pairs with an ExpectedOverlap signal (operator whitelist) |
| IndexImageFile | — | — | — | Dirty-inode computation spawned by ScheduleContentAnalysis (FILES scope). For each dirty inode in "index_image_file": determines image format from file extension, infers role from filename (cover_front/cover_back/other using COVER_FRONT_NAMES/COVER_BACK_NAMES constants), reads dimensions via image_dimensions(), writes to image_info table via UpsertImageInfo DbWriteOp. Clears dirty inodes after processing |
| DetectDeployConflicts | — | DeployConflict | DeployConflict (via hash-based aggregate reconciliation). Uses inode-based signal lookup (signal.inode + metadata path). |
| DetectReleaseOverlaps | Defers DeriveCorpusDeployStatus (DependentAnalysis pipeline barrier) | ReleaseOverlap | ReleaseOverlap (via hash-based aggregate reconciliation). Groups healthy corpus files by album directory (parent of deploy path), partitions by (source_dir, release_dir). Only emits for cross-source overlaps (2+ configured sources targeting the same album directory). Intra-source overlaps are skipped. |
| DeriveDeployHealthSignals | — | LibraryLeftover, LibraryStale | LibraryLeftover, LibraryStale. Handles both audio and sidecar image files. Audio stale: computes expected deploy path from tags. Image stale: finds an audio sibling in the same corpus directory, derives album_dir from its tags, then compares the image's library path against `album_dir/filename`. Uses a lazy `dir_to_album_dir` cache to avoid redundant sibling lookups. Masks stale-conflicts: if a stale file's expected path is already occupied by a different inode, no stale signal is emitted (the LibraryMove would always fail). **Performance**: bulk-loads corpus tags for every library inode in one chunked IN query at the top, replacing per-file `get_tags(inode)` round trips inside `classify_audio_stale` / `lookup_album_dir_from_sibling` / `compute_expected_library_path`. Per-inode fallback retained as defense in depth |
| DeriveCorpusDeployStatus | — | DeployReady, DeployedHealthy, SidecarDeployReady, SidecarDeployConflict | DeployReady, DeployedHealthy (via corpus reconciliation — scalar inode existence check, no hash). DeployedHealthy metadata includes `library_path`. Files deployed at the wrong path (stale) are skipped — DeriveDeployHealthSignals handles those via LibraryStale. Skips conflict losers: files whose computed deploy path is claimed by 2+ corpus files are excluded from DeployReady. Also skips files whose album directory has a ReleaseOverlap signal (release overlap suppression). Phase 4 (sidecars): discovers sidecar images using a single batch query (`get_all_corpus_images`) grouped by directory, then groups candidates by deploy path. Single-candidate paths emit SidecarDeployReady; multi-candidate paths emit SidecarDeployConflict (aggregate signal) and SidecarDeployReady for the tiebreak winner (alphabetically first corpus path). Sidecar phase uses its own dirty-inode gating. **Outer dirty-inode short-circuit**: at entry, reads dirty inodes for `corpus_deploy_status` (marked under TAGS\|FILES\|DEPLOY scope mutations via `witch/execution.rs` Phase 1c). If empty AND prior DeployReady/DeployedHealthy signals exist, returns immediately. Cleared via `clear_all_dirty_inodes` at the end of a successful run. **Performance**: bulk-loads tags for all corpus inodes in one chunked IN query before the per-inode iteration |

### Fingerprinting Limitations

Chromaprint requires a minimum audio duration to generate a usable fingerprint. The algorithm uses 4096-sample frames at 11025 Hz (~0.37 seconds each) with 2/3 overlap, and needs multiple frames to produce meaningful output. In practice, **audio under ~2 seconds cannot be fingerprinted**.

Files that are too short will have empty fingerprints stored in `audio_info`. These files:
- Are excluded from `DetectFingerprintOverlaps` (empty fingerprints are filtered out)
- Cannot participate in duplicate detection via fingerprint matching
- Will emit `CorruptFile` signals during indexing (fingerprint extraction failure)

This primarily affects sound effects, jingles, and very short intros. Such files must be deduplicated manually if needed.

---

## Key Architectural Patterns

### Signal Freshness Optimization

Computations use helper functions to avoid redundant writes:
- `ensure_file_signal_if_missing()` - Only queues write if signal doesn't exist
- `clear_file_signal_if_present()` - Only queues delete if signal exists

### Hash-Based Signal Reconciliation

Bulk detection computations use hash-based set reconciliation:
1. Compute current signal set with content hashes (SipHash of bincode bytes for BLOB types)
2. Fetch existing key→hash map from DB (via `data_hash` column)
3. Clear stale (in DB, not computed), create new, update changed (hash differs), skip unchanged (hash matches)
4. Returns `(cleared, new, updated, unchanged)` counts for logging

This eliminates redundant writes on steady-state cycles where signal data hasn't changed.

### Witness-Based Emission

All signal operations require a witness token (`ComputationWitness` or `MutationExecutionWitness`) ensuring signals are only emitted from authorized contexts.
