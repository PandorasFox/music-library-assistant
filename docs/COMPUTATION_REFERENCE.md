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
- **Analysis phase**: `analysis/mod.rs`, `analysis/schedule.rs`, `analysis/duplicates.rs`, `analysis/tags.rs`, `analysis/deploy.rs`, `analysis/formats.rs`, `analysis/album_art.rs`, `analysis/album_art_info.rs`, `analysis/image_files.rs`, `analysis/inbox_matches.rs`, `analysis/external_matches.rs`, `analysis/release_packing.rs` (4-stage pipeline)
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
| VerifyMtime | Check file modification times for changes |
| VerifyTags | Verify disk tags match indexed tags, emit classification signals |

### Derivation Phase

| Computation | Description |
|-------------|-------------|
| ScheduleSecondLevelDerivations | Orchestrator for per-directory derivation work |
| DeriveDirectorySignals | Derive UnindexedFile, MissingFile, HealthyFile signals |
| UpdateCorpusFileSignals | Lightweight per-file signal update (post-mutation) |
| UpdateLibraryFileSignals | Library-side signal updates |
| UpdateDeploySignals | Update deployment status signals |
| WalkLibrary | Enumerate library directories, spawn per-directory scans |
| ScanLibraryDirectory | Scan library directory, return observed files to Witch |
| ReconcileLibraryFiles | Reconcile observed library files against DB (set reconciliation) |

### Analysis Phase

| Computation | Description |
|-------------|-------------|
| ScheduleContentAnalysis | Orchestrator: spawns all detection computations |
| DetectFingerprintOverlaps | Find tracks with identical fingerprints |
| DetectDuplicateInodes | Find tracks sharing same inode |
| DetectMissingTags | Find tracks missing required tags. Routes ALBUM-only-missing (with ARTIST+TITLE) to MissingAlbumSingleSignal; checks ExpectedMissingTag for suppression |
| DetectMetadataDuplicates | Find tracks with identical tag sets |
| DetectTagCanonicalizations | Find tag canonicalization opportunities |
| DetectInconsistentAlbumArtist | Find inconsistent album_artist across albums. Skips groups where any track has `COMPILATION=0` |
| DetectCompoundTagValues | Orchestrator: spawns DetectCompoundTagsForInode for each dirty corpus inode. Parallelizes detection across worker threads. |
| DetectCompoundTagsForInode | Per-inode: walks the priority-ordered `SplitRule` chain from `TagSplittingOpinions` (separator and collaboration keyword rules). First matching rule wins per tag value. Emits per-file CompoundTag signals. Skips CanonicalTag whitelisted values. |
| DetectShitFormats | Find files with non-Vorbis containers (MP3, M4A, etc). Uses dirty inode tracking — only checks recently (re)indexed inodes |
| DetectInboxCorpusMatches | Find inbox files matching corpus by fingerprint+duration similarity. Pre-computes quality classification (better/equivalent/subpar) using QualityTier with bitrate fuzz |
| DetectInboxTagCanonicity | Compare inbox tag values against corpus vocabulary. Flags inbox values whose normalized form matches a corpus value but whose exact spelling differs. Skips novel values (no corpus equivalent) and CanonicalTag whitelisted values. Full recompute each cycle |
| DetectInboxMissingTags | Detect inbox files missing required tags. Simplified version of DetectMissingTags: no ExpectedMissingTag suppression, no MissingAlbumSingle routing, ALBUM_ARTIST removed from required set when compilation-only. Reuses MissingTagData. Full recompute each cycle |
| DetectInboxCompoundTags | Single-pass compound tag detection for inbox files. Checks collaboration keywords + per-tag separators, enriches matching_parts against corpus vocabulary. No orchestrator/dirty-inode tracking (inbox is small). Full recompute each cycle |
| DetectPathTagMismatches | Detect files whose paths don't match their source dir's path-tag schema. Extracts tag values from path structure, compares against DB tags (case-insensitive). Emits per-file PathTagMismatch signals |
| DeriveExternalMatches | Derive external match signals from AcoustID results. Compares recording metadata (title, artist, album) against corpus tags using raw string equality. Emits per-file ExternalMatch signals with classification and diffs |
| PackReleases | **Stage 1 orchestrator** for the release bin-packing pipeline. Loads AcoustID external matches, filters by min_confidence and duration_tolerance_pct, identifies candidate releases, loads tracklists and locale-resolved artist names, writes session manifest to `release_packing_manifest`, truncates intermediate tables. Uses ALL recordings per inode (not just the best-confidence one), so inodes with multiple AcoustID recordings at similar confidence are linked to all possible releases. Candidates are deduplicated per (release_id, inode) keeping the highest-confidence recording. Writes `dir_file_count` (total audio files in each candidate's directory) to the candidates table. Spawns N `ScoreReleaseCandidates` (one per release). Defers `ComputeReleaseMappings` as barrier-separated phase (which chains the remaining stages). Manual trigger only |
| ScoreReleaseCandidates | **Stage 2** (N parallel). Per-release directory-constrained candidate scoring and elimination matching. Loads release tracklist, finds corpus inodes matched via AcoustID recordings linked to this release. **Directory selection** constrains all packing to a single target directory (or sibling directories for multi-medium releases). Single-medium: prefers directory where file count matches track count with the most AcoustID candidates, tiebreak on score sum. Multi-medium: tries sibling directory bijection (each dir's file count matches a medium's track count, same parent), falls back to single-dir. **Candidate scoring** uses configurable `candidate_weights` with 6 independent dimensions (default: AcoustID confidence 0.30, duration match 0.30, title match 0.10, artist match 0.05, album match 0.05, track number match 0.20). Optimal per-release assignment via Hungarian algorithm (Kuhn-Munkres) on directory-filtered candidates only. After AcoustID scoring, performs per-release **elimination matching** for unfilled slots within the same target directory using configurable `elimination_weights` with the same 6 dimensions (default: duration match 0.25, title match 0.30, artist match 0.0, album match 0.05, track number match 0.40; no AcoustID). Elimination uses two phases: title pre-assignment (threshold 0.95, 1:1 unambiguous matches only) followed by Hungarian on remaining pairs (no score threshold — directory constraint provides the quality gate). Artist match defaults to 0 in elimination because rip artist tags often diverge from MB release-level credits (e.g., individual composers vs game studio). Both weight sets are configurable under `release-packing` in config.kdl. Writes all candidates + optimal flags to `release_packing_scores` table with `match_method` column (`AcoustId` or `Elimination`). See `docs/RELEASE_PACKING_ALGORITHM.md` for full algorithm reference |
| ComputeReleaseMappings | **Stage 3a** (after barrier). Orchestrator for tiered MIS resolution. Loads optimal proposals from `release_packing_scores` and directory metadata from `release_packing_candidates`. Classifies each proposal into a quality tier: **Perfect** (all slots filled, 1:1 directory↔release mapping, no leftover files; multi-medium aware), **FullMatch** (all slots filled but cross-directory or extras), **Incomplete** (some but not all slots filled), **Single** (single-track release). With directory-constrained packing, cross-directory scattering is structurally impossible. Packages state and defers `MapPerfectReleases` |
| MapPerfectReleases | **Stage 3b** (after barrier). MIS on Perfect proposals. Selects non-conflicting proposals maximizing corpus inode coverage. Exhaustive bitmask enumeration for components ≤25, branch-and-bound for larger. Defers `MapFullMatchReleases` |
| MapFullMatchReleases | **Stage 3c** (after barrier). MIS on FullMatch proposals. Proposals only eligible if entire inode set unclaimed by prior rounds. Defers next stage dynamically: `MapIncompleteReleases` (default) or `MapSingleReleases` (when `singles-before-incompletes` is true) |
| MapIncompleteReleases | **Stage 3d** (after barrier). MIS on Incomplete proposals. Proposals enter with unclaimed portion of inode set. Defers dynamically to the other of Singles/EmitSignals |
| MapSingleReleases | **Stage 3e** (after barrier). Per-inode-best for Singles (no MIS needed). Defers dynamically to the other of Incomplete/EmitSignals |
| EmitReleasePackingSignals | **Stage 3f** (after barrier). Final MIS stage. Emits per-inode `ReleasePacking` signals for ALL rounds. Emits `PackingKnot` signals for dense conflict graph components captured during MIS rounds. Records `pending_acoustid_submissions` for elimination winners. Defers `AnalyzeReleaseGaps` |
| AnalyzeReleaseGaps | **Stage 4** (after barrier). Post-resolution gap analysis. Identifies unmatched corpus tracks (both from candidate pipeline and fingerprinted files with no AcoustID match) → `UnmatchedCorpusTrack` signals. Computes per-release filled slots from actual inode→release assignments (not optimal scores) → `UnfilledReleaseSlot` signals. Emits per-release `PackedRelease` aggregate signals with typed category (Perfect/FullMatch/Single/Incomplete). **Coverage filtering:** suppresses `PackedRelease` incomplete and `UnfilledReleaseSlot` signals for releases where every candidate inode is already assigned to a full-match release (fully-covered releases represent cached MB entries, not real gaps). Detects near-miss patterns ((n-1)/n tracks matched from same directory containing n audio files) → `NearMissRelease` signals |
| DetectDiscExtractions | Detect extractable disc numbers from ALBUM and TRACKNUMBER tags. Pass 1 (album): scans ALBUM tags for `,?\s*disc\s+(\d+)\s*$` pattern. Pass 2 (track number): scans TRACKNUMBER for `^([A-Za-z]+)(\d+)$`, groups by release context (album+album_artist), only emits if group has ≥2 files. Scans both corpus and inbox. Emits DiscExtraction aggregate signals |
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
| ScanCorpusDirectory | VerifyMtime (if mtime changed, normal mode) or VerifyTags + VerifyAudio (all indexed, if `force_check=true`) | FileInCorpus (corpus zone), FileInInbox (inbox zone) | — | Also indexes directory entry (is_dir=1) in files table with read guard (skips write if entry already matches by zone+inode+mtime). Returns observed inodes to the Witch via Result (accumulated in tick(), consumed by queue_derivation_computations()). |
| VerifyMtime | VerifyTags (if mtime differs) | — | — |
| VerifyTags | — | OutOfBandTagConflict, OutOfBandTagSync, MtimeOnlyMismatch, CorruptFile | OutOfBandTagConflict, OutOfBandTagSync, MtimeOnlyMismatch (mutual exclusion) |
| VerifyAudio | — | CorruptFile | CorruptFile (if audio valid) |

### Derivation Phase Computations

| Computation | Spawns | Signals Emitted | Signals Cleared |
|-------------|--------|-----------------|-----------------|
| ScheduleSecondLevelDerivations | WalkLibrary × N | MissingDirectory | MissingDirectory (if dir exists again) |
| DeriveCorpusSignals | — | UnindexedFile, MissingFile, HealthyFile | UnindexedFile, MissingFile, HealthyFile (stale); skips HealthyFile for OOB-flagged files. **GC backstop**: clears orphaned signals for inodes not in disk ∪ index. Receives observed inodes from the Witch (accumulated from ScanCorpusDirectory results). |
| DeriveInboxSignals | — | InboxUnindexed, InboxHealthy | InboxUnindexed (stale). **Cascade-drop**: for indexed inbox inodes no longer on disk, drops all inbox state via DropInboxFileState (inbox_tags, files zone='inbox', FileInInbox, InboxUnindexed, InboxHealthy, InboxCorpusMatch, MovedFile). Disk presence is sole authority — GC backstop uses disk_set only (not disk ∪ indexed). Receives observed inodes from the Witch (accumulated from ScanCorpusDirectory results). |
| UpdateCorpusFileSignals | — | FileInCorpus, UnindexedFile, MissingFile, HealthyFile | FileInCorpus, UnindexedFile, MissingFile, HealthyFile |
| UpdateLibraryFileSignals | — | — | LibraryLeftover, LibraryStale |
| UpdateDeploySignals | — | DeployedHealthy | DeployReady, LibraryLeftover, LibraryStale |
| WalkLibrary | ScanLibraryDirectory × N | — | — |
| ScanLibraryDirectory | — | — | — | Returns observed library files to the Witch via Result (accumulated in tick()). No direct DB writes. |
| ReconcileLibraryFiles | — | — | — | Set reconciliation: compares observed files against DB. Upserts new/changed files, deletes stale files, skips unchanged. Queued by the Witch after derivation stage 1 drains (two-stage derivation transition). |

### Analysis Phase Computations

| Computation | Spawns | Signals Emitted | Signals Cleared |
|-------------|--------|-----------------|-----------------|
| ScheduleContentAnalysis | All detection computations (except fingerprint-dependent: AnalyzeFingerprintOverlaps and DetectCrossSourceOverlaps are deferred phases of DetectFingerprintOverlaps), IndexImageFile (FILES scope) | — | — |
| DetectFingerprintOverlaps | Defers AnalyzeFingerprintOverlaps + DetectCrossSourceOverlaps (DependentAnalysis pipeline barrier) | FingerprintOverlap | FingerprintOverlap (stale) |
| DetectDuplicateInodes | — | DuplicateInode | DuplicateInode (stale) |
| DetectMissingTags | — | MissingTag, MissingAlbumSingleSignal | MissingTag (via hash-based reconciliation), MissingAlbumSingleSignal (via hash-based reconciliation). Files with ALBUM missing but ARTIST+TITLE present are routed to MissingAlbumSingleSignal (keyed by lowercased artist) instead of MissingTag. Checks ExpectedMissingTag to suppress known-acceptable missing-album inodes |
| DetectMetadataDuplicates | — | MetadataDuplicate | MetadataDuplicate (via hash-based reconciliation) |
| DetectTagCanonicalizations | — | TagCanonicity | TagCanonicity (via hash-based reconciliation). Loads `strip_album_format_suffixes` from config for album collision detection. Skips collision groups where any variant has a CanonicalTag signal |
| DetectCompoundTagValues | DetectCompoundTagsForInode (per inode) | — | CompoundTag (all, before spawning) |
| DetectCompoundTagsForInode | — | CompoundTag (per-file) | — |
| DetectInconsistentAlbumArtist | — | InconsistentAlbumArtist | InconsistentAlbumArtist (via hash-based reconciliation). Groups with `COMPILATION=0` on any track are suppressed (no signal emitted) |
| DetectShitFormats | — | ShitFormat | ShitFormat (via dirty inode tracking — only processes recently indexed inodes) |
| AnalyzeFingerprintOverlaps | — | SubparDuplicate, RedundantDuplicate | SubparDuplicate (via hash-based corpus reconciliation), RedundantDuplicate (via hash-based aggregate reconciliation). Uses enum-based equivalence-class partitioning (QualityTier = FormatClass + metric). Best tier with >1 file → RedundantDuplicate; lower tiers → SubparDuplicate with reason (SubparFormat, SubparBitrate, SubparSampleRate). Re-release elision: album checked before ISRC when catalog numbers absent. Skips fingerprint groups with an ExpectedDuplicate signal (operator whitelist). Skips groups entirely within a single source dir that has `interior-dupes false` in config |
| DetectInboxCorpusMatches | — | InboxCorpusMatch | InboxCorpusMatch (via hash-based corpus reconciliation). For each inbox file with fingerprint, finds corpus files within duration tolerance with similarity above threshold. Pre-computes quality classification (Better/Equivalent/Subpar) via QualityTier comparison with inbox_bitrate_fuzz_percent. Classification stored as SQL column + bincode BLOB field. `better` files pass through to inbox organize |
| DetectInboxTagCanonicity | — | InboxTagCanonicity | InboxTagCanonicity (via hash-based aggregate reconciliation). For each tag field (artist, album_artist, album, genre): normalizes inbox values, finds corpus matches, skips exact matches and CanonicalTag whitelisted values. Data stored as bincode BLOB |
| DetectInboxMissingTags | — | InboxMissingTag | InboxMissingTag (via hash-based aggregate reconciliation). Groups inbox files by album/directory, checks against configured required_tags. No suppression, no album-single routing. Data reuses MissingTagData bincode BLOB |
| DetectInboxCompoundTags | — | InboxCompoundTag | InboxCompoundTag (per-inode write/clear). Single-pass over inbox healthy inodes. Checks collaboration keywords + per-tag separators, enriches matching_parts against corpus vocabulary. Cleans up signals for inodes no longer healthy |
| DetectPathTagMismatches | — | PathTagMismatch | PathTagMismatch (via hash-based corpus reconciliation). For each corpus audio file with a matching source dir schema (direct or inherited), strips source dir prefix and extension, runs schema extraction, compares extracted tags against DB tags (case-insensitive). Emits StructureMismatch or ValueMismatch signals |
| DetectDiscExtractions | — | DiscExtraction | DiscExtraction (via hash-based aggregate reconciliation). Pass 1: scans ALBUM tags from corpus_tags and inbox_tags for `,?\s*disc\s+(\d+)\s*$` pattern. Pass 2: scans TRACKNUMBER tags for `^([A-Za-z]+)(\d+)$` prefix pattern, groups by release context (album+album_artist), only emits if group has ≥2 files |
| DeriveExternalMatches | — | ExternalMatch | ExternalMatch (via hash-based corpus reconciliation). For each corpus inode with AcoustID matches, parses stored raw response JSON, compares recording title/artist/album against corpus tags (raw string equality), classifies as ExactMatch/ContentDiff/MetadataOnly. Triggered by EXTERNAL, TAGS, or FILES scope |
| PackReleases | ScoreReleaseCandidates × N; defers ComputeReleaseMappings (which chains remaining stages) | — | — | Stage 1 orchestrator. Loads AcoustID matches, uses ALL recordings per inode (deduped per (release_id, inode) keeping highest-confidence), writes manifest to `release_packing_manifest`, writes `dir_file_count` to candidates table, truncates intermediate tables, spawns per-release scorers. Clears stale ReleasePacking signals if no matches. Manual trigger only |
| ScoreReleaseCandidates | — | — | — | Stage 2. Per-release directory-constrained scoring + elimination matching. 6 independent scoring dimensions: acoustid_confidence, duration_match, title_match, artist_match, album_match, track_number_match. Directory selection constrains all packing to a single target directory (or sibling directories for multi-medium via density-based mapping). Optimal assignment via Hungarian algorithm on directory-filtered candidates. Then per-release elimination within the same target directory(ies): title pre-assignment (0.95 threshold, 1:1 only) followed by Hungarian on remaining pairs (no score threshold). Per-medium affinity enforced for multi-medium releases. Writes all candidates + optimal picks to `release_packing_scores` table with `match_method` column (AcoustId or Elimination). No signals emitted (intermediate data only) |
| ComputeReleaseMappings | Defers MapPerfectReleases | — | — | Stage 3a orchestrator. Classifies proposals into quality tiers (Perfect, FullMatch, Incomplete, Single), packages state for MIS rounds |
| MapPerfectReleases | Defers MapFullMatchReleases | — | — | Stage 3b. MIS on Perfect pool (coverage-based objective) |
| MapFullMatchReleases | Defers Incomplete or Singles (config-dependent) | — | — | Stage 3c. MIS on FullMatch pool |
| MapIncompleteReleases | Defers Singles or EmitSignals (config-dependent) | — | — | Stage 3d. MIS on Incomplete pool (partial inode sets) |
| MapSingleReleases | Defers Incomplete or EmitSignals (config-dependent) | — | — | Stage 3e. Per-inode-best for singles |
| EmitReleasePackingSignals | Defers AnalyzeReleaseGaps | ReleasePacking, PackingKnot | ReleasePacking (via hash-based corpus reconciliation), PackingKnot (via hash-based aggregate reconciliation). Stage 3f. Emits ReleasePacking signals for ALL rounds with mixed match methods (AcoustId or Elimination). Emits PackingKnot signals for dense conflict graph components captured during MIS rounds (knots resolved greedily). Records pending AcoustID submissions for elimination winners to `pending_acoustid_submissions` table |
| AnalyzeReleaseGaps | — | UnmatchedCorpusTrack, UnfilledReleaseSlot, PackedRelease, NearMissRelease | UnmatchedCorpusTrack (via hash-based corpus reconciliation), UnfilledReleaseSlot (via hash-based aggregate reconciliation), PackedRelease (via hash-based aggregate reconciliation), NearMissRelease (via hash-based aggregate reconciliation). Stage 4. Emits UnmatchedCorpusTrack for inodes that were scored but not assigned AND fingerprinted corpus files with no AcoustID match. Builds filled_slots from actual inode→release assignments (not optimal scores). Emits PackedRelease per release with typed category (key prefix: full_match/single/near_miss/incomplete). Coverage filtering: suppresses PackedRelease near-miss/incomplete and UnfilledReleaseSlot signals for releases where every candidate inode is already assigned to a full-match release (fully-covered releases are cached MB entries, not real gaps). Only emits UnfilledReleaseSlot for releases with at least one filled slot. Near-miss requires (n-1)/n tracks matched, all from same directory containing exactly n audio files |
| DetectCrossSourceOverlaps | — | CrossSourceOverlap (keyed by sorted source pair, e.g., "bandcamp\|indie") | CrossSourceOverlap (via hash-based aggregate reconciliation). Skips source pairs with an ExpectedOverlap signal (operator whitelist) |
| IndexImageFile | — | — | — | Dirty-inode computation spawned by ScheduleContentAnalysis (FILES scope). For each dirty inode in "index_image_file": determines image format from file extension, infers role from filename (cover_front/cover_back/other using COVER_FRONT_NAMES/COVER_BACK_NAMES constants), reads dimensions via image_dimensions(), writes to image_info table via UpsertImageInfo DbWriteOp. Clears dirty inodes after processing |
| DetectDeployConflicts | — | DeployConflict | DeployConflict (via hash-based aggregate reconciliation). Uses inode-based signal lookup (signal.inode + metadata path). |
| DetectReleaseOverlaps | Defers DeriveCorpusDeployStatus (DependentAnalysis pipeline barrier) | ReleaseOverlap | ReleaseOverlap (via hash-based aggregate reconciliation). Groups healthy corpus files by album directory (parent of deploy path), partitions by (source_dir, release_dir). Only emits for cross-source overlaps (2+ configured sources targeting the same album directory). Intra-source overlaps are skipped. |
| DeriveDeployHealthSignals | — | LibraryLeftover, LibraryStale | LibraryLeftover, LibraryStale. Handles both audio and sidecar image files. Audio stale: computes expected deploy path from tags. Image stale: finds an audio sibling in the same corpus directory, derives album_dir from its tags, then compares the image's library path against `album_dir/filename`. Uses a lazy `dir_to_album_dir` cache to avoid redundant sibling lookups. Masks stale-conflicts: if a stale file's expected path is already occupied by a different inode, no stale signal is emitted (the LibraryMove would always fail). |
| DeriveCorpusDeployStatus | — | DeployReady, DeployedHealthy, SidecarDeployReady, SidecarDeployConflict | DeployReady, DeployedHealthy (via corpus reconciliation — scalar inode existence check, no hash). DeployedHealthy metadata includes `library_path`. Files deployed at the wrong path (stale) are skipped — DeriveDeployHealthSignals handles those via LibraryStale. Skips conflict losers: files whose computed deploy path is claimed by 2+ corpus files are excluded from DeployReady. Also skips files whose album directory has a ReleaseOverlap signal (release overlap suppression). Phase 4 (sidecars): discovers sidecar images using a single batch query (`get_all_corpus_images`) grouped by directory, then groups candidates by deploy path. Single-candidate paths emit SidecarDeployReady; multi-candidate paths emit SidecarDeployConflict (aggregate signal) and SidecarDeployReady for the tiebreak winner (alphabetically first corpus path). Uses dirty-inode gating: skips entirely when no dirty inodes exist and signals are already populated; always uses full reconciliation when running (cross-directory conflicts require global view). |

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
