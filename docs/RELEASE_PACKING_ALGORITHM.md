# Release Packing Algorithm Reference

The release packing pipeline assigns corpus audio files to MusicBrainz release track slots via a 4-stage process: candidate identification, directory-constrained scoring, tiered conflict resolution, and gap analysis. All packing is constrained to a single directory (or sibling directories for multi-medium releases).

Source: `src/meta/computations/analysis/release_packing.rs`

---

## Pipeline Overview

| Stage | Computation | Purpose |
|-------|------------|---------|
| 1 | PackReleases { incremental } | Identify candidates, write manifest, spawn per-release scorers. Incremental mode skips solved releases |
| 2 | ScoreReleaseCandidates (×N) | Directory selection, AcoustID scoring, elimination matching |
| 3a | ComputeReleaseMappings | Classify proposals into quality tiers |
| 3b–3e | Map{Perfect,FullMatch,Incomplete,Single}Releases → N×ResolvePackingComponent | Tiered orchestrators: find connected components, spawn parallel per-component MIS solvers. Knots extracted and resolved greedily by orchestrators. Isolated nodes emitted directly. (Incomplete/Single order configurable) |
| 4 | EmitUnmatchedSignals { incremental } | Emit signals for unmatched corpus tracks and unfilled release slots. Incremental mode excludes MB-tagged inodes |

Stages are barrier-separated: each defers the next via `SharedMappingState`, ensuring serial execution managed by the Witch's computation scheduler.

---

## Stage 1: PackReleases (Orchestrator)

**Trigger:** Manual only (operator requests release packing). Auto-triggered after external fetch (incremental mode).

**Parameter:** `incremental: bool` — when true, solved releases are skipped.

1. Load all AcoustID external matches for corpus files
2. Filter recordings by `min_confidence` (from config). Duration is not filtered — it is a scoring dimension in Stage 2.
3. Parse release tracklists from MB cache (locale-resolved artist names)
4. Write manifest rows (`release_id`, `total_tracks`, `media_count`, `title`, `artist`) to `packing_manifest` table
5. Deduplicate candidates per `(release_id, inode)` — keep highest-confidence recording
6. Write candidate rows to `release_packing_candidates` intermediate table
7. **Pinned release injection**: For each source dir with a `pinned_release` configured, add the pinned release ID to `all_release_ids` to ensure its tracklist is fetched. For inodes in that dir that have no AcoustID candidate for the pinned release, inject a synthetic candidate row (`confidence = 1.0`, empty `recording_id`). This guarantees every file in a pinned dir participates in scoring for the pinned release regardless of fingerprint match quality.
8. **Incremental filtering** (when `incremental=true`): Load all MB-tagged inodes (files with both configured MB track + release tags). Group candidates by release_id. A release is "solved" if ALL its candidate inodes are MB-tagged AND the release is NOT pinned. Remove all candidates for solved releases. This skips rescoring already-applied matches, typically cutting the pipeline to ~1/3 of releases.
9. Spawn N `ScoreReleaseCandidates` computations (one per release with candidates)
10. Defer `ComputeReleaseMappings` via barrier (threads `incremental` through)

**Key data:**
- `CorpusFileInfo`: `parent_dir`, tags (`TITLE`/`ARTIST`/`ALBUM`/`TRACKNUMBER`), `duration_ms`
- `dir_file_count`: total audio files per directory (written to candidates table)
- `pinned_release`: `Option<String>` on `SourceDir` in `dirs.kdl` config — operator-asserted release MBID for a directory

---

## Stage 2: ScoreReleaseCandidates (Parallel)

Each instance handles one release. Four phases execute sequentially.

### Phase 1: Exhaustive Per-Directory Scoring

All packing is constrained to target directory(ies) selected by `score_all_directories()`. This runs Hungarian assignment for every candidate directory and picks the one producing the highest total assignment score — replacing the former heuristic-based `select_target_directory()` that could produce non-deterministic results.

**Algorithm:**
1. Collect all unique directories containing candidate inodes
2. **Pinned release**: If this release is pinned by one or more source dirs, `dir_candidate_inodes` is restricted to only those pinned dirs. Scoring and directory selection proceed normally within that constraint. For multi-medium pinned releases, each pinned dir maps to a medium in the `PerMedium` target via the standard sibling-dir mapping — coalescing works naturally without special-casing.
3. **Multi-medium** (`media.len() > 1`): try `find_sibling_dir_mapping()` to detect sibling directories (same parent) with candidates for different media. If found, filter candidates to the sibling set and run Hungarian — record as a candidate result.
4. **Per-directory**: for each individual directory, filter candidates to that directory, run Hungarian, compute total assignment score.
5. Select the `(TargetDirs, optimal_pairs)` with:
   - Most assigned slots (primary)
   - Highest total assignment score (secondary)
   - Lexicographic smallest directory path (deterministic tiebreak)

After selection, all candidates are filtered to the winning directory set:
```
candidates.retain(|c| target_dirs.contains(&corpus_info[c.inode].parent_dir))
```

The optimal pairs from Phase 1 are reused directly — no second Hungarian call is needed.

### Phase 2: AcoustID Scoring (Hungarian)

Each `CandidateAssignment` is a `(inode, track_slot)` pair scored across 6 dimensions (see [Scoring Dimensions](#scoring-dimensions)). The Hungarian algorithm (Kuhn-Munkres) finds the optimal bipartite matching maximizing total composite score.

- Input: directory-filtered candidates with scores computed via `candidate_weights`
- Output: optimal 1:1 assignment of inodes to track slots
- All candidates written to `release_packing_scores` with `is_optimal` flag

### Phase 3: Title Pre-assignment (Elimination Phase 1)

Scans unassigned audio files in the target directory(ies). For each `(file, unfilled_slot)` pair, computes title similarity.

**Per-medium affinity:** For `PerMedium` targets, only file-slot pairs where the file's parent directory matches the slot's medium's assigned directory are considered. This prevents cross-medium contamination (e.g., Disc 1 files stealing Medium 2 slots).

**Assignment rule:** Lock in a match only when both the file and the slot have exactly one candidate above the threshold (0.95). This prevents track-number ordering from stealing slots with clear title matches when rip numbering diverges from MB.

### Phase 4: Elimination Matching (Elimination Phase 2)

Runs Hungarian on remaining `(unassigned_files × unfilled_slots)` using `elimination_weights`. All Hungarian-assigned pairs are accepted — no score threshold is applied, since the directory constraint already guarantees files are from the correct directory. Hungarian picks the optimal assignment; rejecting low-scoring pairs would only create gaps.

**Per-medium affinity:** For `PerMedium` targets, cross-medium file-slot pairs receive a prohibitive cost (`1e9`) in the Hungarian matrix, ensuring each medium's slots are only filled by files from that medium's assigned directory.

Elimination winners record `(fingerprint_hex, recording_id)` for the pending AcoustID submission queue.

---

## Scoring Dimensions

Six independent dimensions, each normalized to `[0.0, 1.0]`:

| Dimension | Candidate Default | Elimination Default | Description |
|-----------|:-:|:-:|-------------|
| `acoustid_confidence` | 0.30 | 0.00 | Direct AcoustID match confidence |
| `duration_match` | 0.30 | 0.25 | Duration ratio within tolerance → linear scale; 0.5 if either duration missing |
| `title_match` | 0.10 | 0.30 | Normalized Levenshtein of corpus TITLE vs MB track/recording title (max of both) |
| `artist_match` | 0.05 | 0.00 | Normalized Levenshtein of corpus ARTIST vs release artist. Zero in elimination: rip tags diverge from MB release-level credits |
| `album_match` | 0.05 | 0.05 | Normalized Levenshtein of corpus ALBUM vs release title |
| `track_number_match` | 0.20 | 0.40 | Exact match of corpus TRACKNUMBER to MB track position (1.0 or 0.0) |

**Composite score:**
```
score = Σ(weight_i × dimension_i)
```

Both weight sets and all packing parameters are configurable under `release-packing` in `config.kdl`.

---

## Stage 3: Tiered MIS Conflict Resolution

### 3a: ComputeReleaseMappings

Loads optimal picks from scoring table. Groups into per-release `Proposal` objects. Classifies each by `classify_proposal()`:

| ProposalTier | Criteria |
|-------------|----------|
| Perfect | All slots filled, single-directory purity (per-medium for multi-medium), no leftover files in directory |
| FullMatch | All slots filled but directory has extra files or minor purity issues |
| Incomplete | Some but not all slots filled |
| Single | Single-track release |

Proposals are sorted into 4 pools and processed in priority order.

**Pinned release handling in Stage 3a:**

Before MIS rounds begin, pinned proposals are pre-accepted:

1. **Conflict detection**: If a release is pinned by more dirs than it has media (e.g., two dirs both pinning a single-medium release), it is a conflict. `PinnedReleaseConflict` is emitted for the release and it is skipped entirely — neither dir gets packed.
2. **Pre-acceptance**: Non-conflicted pinned proposals are accepted immediately and their inodes are marked claimed before any MIS round runs.
3. **Conflict rejection**: During subsequent MIS rounds, any non-pinned proposal that overlaps claimed (pinned) inodes is rejected outright — pinned assignments cannot be displaced.

This ensures operator-asserted release assignments are always honored, at the cost of a hard stop when the operator's configuration is self-contradictory.

### 3b–3e: MIS Rounds

Each round selects non-conflicting (inode-disjoint) proposals to maximize corpus coverage.

**Round flow (FullMatch, Incomplete):**
1. **Cull:** Discard proposals that lost any inode to prior rounds
2. **Dedup:** Group by sorted inode signature, keep best-scorer per group. Losers are captured as `AlternativeRelease` siblings of the keeper for alternative detection
3. **Knot extraction:** Build conflict graph (proposals sharing inodes are adjacent). Connected components with `proposals/inodes >= knot_ratio` (default 3.0) or `size > knot_size_limit` (default 50) are extracted as knots. **Discography reduction** (when `allow-resolve-knots-with-discographies` is true, default): if any proposals in the knot cover ALL contested inodes, they are emitted as normal picks and the knot is fully resolved — no knot signal emitted, losers silently dropped. Otherwise, the knot is emitted as an unresolved `PackingKnotSignal` for manual review — no picks are emitted and contested inodes remain unclaimed.
4. **MIS solve:** Remaining clean components enter exact MIS:
   - Components ≤25 proposals: exhaustive bitmask enumeration (2^k subsets)
   - Components >25: branch-and-bound with coverage/score objective

**Perfect round** skips cull/dedup/knots — proposals are strict (all inodes must be unclaimed).

**Singles round:** per-inode best score, no MIS needed (no multi-inode conflicts).

**Round ordering** after FullMatch is configurable via `singles-before-incompletes`:
- `true` (default): Singles → Incomplete
- `false`: Incomplete → Singles

When singles run first, single-track releases claim inodes before the expensive Incomplete MIS round, preventing single-file incompletes from competing there.

Each tier orchestrator finds connected components in the conflict graph, then: (1) isolated nodes (size 1) are emitted directly, (2) knots are extracted and resolved greedily with signals emitted by the orchestrator, (3) clean multi-node components are spawned as parallel `ResolvePackingComponent` computations that independently build local conflict graphs and solve MIS. Inter-tier inode tracking reads assigned inodes from the DB (`signal_release_packing` table). AcoustID submissions for elimination winners are recorded per-component.

All packing signal tables (`signal_packed_release`, `signal_release_packing`, `signal_packing_knot`, `signal_alternative_release_packing`, `signal_various_artists_override`) are bulk-cleared at pipeline start (ComputeReleaseMappings, Stage 3a).

### Alternative Release Detection

Within any MIS round, multiple proposals may cover the **exact same inode set** — different pressings, editions, or regional variants that pack identically against the same corpus files. These are trivially interchangeable: swapping one for another changes zero inode assignments.

**Detection:** During dedup-by-inode-signature (partial tiers) and via bookkeeping (Perfect tier), proposals are grouped by sorted inode signature. For each group with >1 member, the best-scorer is the keeper and the rest become `AlternativeRelease` siblings. Siblings are attached to `ComponentData` and flow through to all emission paths.

**Scope invariant:** Alternatives are scoped per **individual winning release**, not per aggregate "winning set." A compilation covering `{1,2,3,4,5,6}` is never an alternative to a release covering `{1,2,3}` — only proposals with a byte-identical inode signature qualify.

**Emission paths:**
1. **Isolated proposals** — siblings passed directly to emitter
2. **Discography reduction** (knot path) — sig groups built among covering proposals; non-selected proposals with same signature as a selected one become its alternatives
3. **Standard knot resolution** — no alternatives (no winners picked)
4. **MIS component solvers** — siblings carried in `ComponentData::signature_siblings`, emitted per winner

### Various Artists Override

When a winning release has "Various Artists" as its album artist, a more specific artist name is suggested via two-tier lookup:

1. **Exact alternatives (ExactAlternative):** Scan signature siblings for the most frequent non-VA artist. Emitted if found.
2. **Competing proposals (CompetingProposal):** Fallback if no non-VA name in exact alternatives. Scans all proposals in scope (component proposals for MIS path, covering proposals for discography path, manifest for isolated) for non-VA artists on releases overlapping the winner's inodes. Most frequent wins.

### PackedRelease (Categories)

| PackedReleaseCategory | Key Prefix | Criteria |
|-----------------------|------------|----------|
| Perfect | `perfect:` | All slots filled, all via AcoustID |
| FullMatch | `full_match:` | All slots filled, at least one via elimination |
| Single | `single:` | Single-track release |
| Incomplete | `incomplete:` | Partial coverage |
| LowConfidence | `low_confidence:` | FullMatch/Incomplete downgraded: AcoustID ratio < threshold AND avg album_match < threshold |

### Low-Confidence Detection

After tier classification, FullMatch and Incomplete proposals are checked for low-confidence indicators. When both conditions are met simultaneously:

1. **AcoustID ratio** (rows with `match_method == AcoustId` / total rows) is below `low-confidence-max-acoustid-ratio` (default 0.25)
2. **Average album_match** (mean of `album_match` from all row score breakdowns) is below `low-confidence-max-album-match` (default 0.30)

...the proposal's category is downgraded from FullMatch/Incomplete to **LowConfidence**. This catches false matches where a small number of garbage AcoustID hits led elimination to fill the remaining slots on an unrelated release (e.g., OutRun 20th Anniversary Box mapped to Bayonetta OST). Perfect and Single tiers are never downgraded.

---

## Stage 4: EmitUnmatchedSignals

**Parameter:** `incremental: bool` — threaded from Stage 1.

Post-resolution analysis producing 2 signal types:

### UnmatchedCorpusTrack
Emitted for inodes with AcoustID recording matches but no release assignment. Also covers fingerprinted files with no AcoustID match at all.

**Incremental mode:** MB-tagged inodes are excluded from unmatched detection. In incremental mode, solved releases were skipped in Stage 1 and their inodes have no `ReleasePacking` signals — without this filter they would be falsely flagged as unmatched. This exclusion is harmless in full mode (those inodes would have assignments anyway).

### UnfilledReleaseSlot
Empty track slots in partially-assigned releases. Only emitted for releases with `filled_count > 0`.

**Coverage filtering:** Suppresses `UnfilledReleaseSlot` signals for releases where every candidate inode is already assigned to a full-match release (cached MB entries, not real gaps).

---

## ProposalTier vs PackedReleaseCategory

These are distinct classification systems used at different stages:

- **ProposalTier** (Stage 3a): Input classification determining which MIS round pool a proposal enters. Based on slot coverage, directory structure, and leftover file counts.

- **PackedReleaseCategory** (Stage 3b-3e): Output classification for the signal, derived directly from `ProposalTier` (1:1 mapping: Perfect→Perfect, FullMatch→FullMatch, Incomplete→Incomplete, Single→Single). Emitted by ResolvePackingComponent (for multi-node components) or directly by tier orchestrators (for isolated nodes and knot winners). This is what the UI displays.

---

## Key Config Fields

All configurable under `release-packing` in `config.kdl`:

| Field | Default | Purpose |
|-------|---------|---------|
| `title-preassign-threshold` | 0.95 | Title pre-assignment: high confidence, 1:1 only |
| `packing-knot-ratio` | 3.0 | Knot extraction: proposals/inodes threshold (0 to disable) |
| `packing-knot-size-limit` | 50 | Max component size before forced knot extraction (0 to disable) |
| `singles-before-incompletes` | true | Run singles MIS round before incompletes |
| `allow-resolve-knots-with-discographies` | true | Reduce knots to covering proposals when possible |
| `low-confidence-max-acoustid-ratio` | 0.25 | Max AcoustID-matched fraction for low-confidence downgrade |
| `low-confidence-max-album-match` | 0.30 | Max avg album_match score for low-confidence downgrade |
| ~~`ELIMINATION_SCORE_THRESHOLD`~~ | Removed | No threshold — directory constraint provides the quality gate |

---

## Data Flow

```
AcoustID matches (signal_external_match)
        │
        ▼
  ┌─────────────┐     ┌──────────────────────┐
  │ Stage 1:    │────▶│ release_packing_      │
  │ PackReleases│     │ candidates + manifest │
  └─────────────┘     └──────────┬───────────┘
                                 │
                    ┌────────────┼────────────┐
                    ▼            ▼            ▼
              ┌──────────┐ ┌──────────┐ ┌──────────┐
              │ Stage 2  │ │ Stage 2  │ │ Stage 2  │  (× N releases)
              │ Score +  │ │ Score +  │ │ Score +  │
              │ Eliminate │ │ Eliminate │ │ Eliminate │
              └────┬─────┘ └────┬─────┘ └────┬─────┘
                   │            │            │
                   └────────────┼────────────┘
                                ▼
                   ┌────────────────────────┐
                   │ release_packing_scores │
                   └───────────┬────────────┘
                               ▼
                   ┌────────────────────────┐
                   │ Stage 3a: Classify     │
                   │ into proposal tiers    │
                   └───────────┬────────────┘
                               │
              ┌────────┬───────┼───────┐
              ▼        ▼       ▼       ▼
          Perfect  FullMatch  Inc.   Single
           (3b)     (3c)     (3d)    (3e)
              │        │       │       │
              │        │       └───┬───┘  ← order configurable
              └────────┴───────────┘
                               ▼
                   ┌────────────────────────┐
                   │ Stage 4: Unmatched     │
                   │ → UnmatchedCorpusTrack │
                   │ → UnfilledReleaseSlot  │
                   └────────────────────────┘
```
