# Release Packing Algorithm Reference

The release packing pipeline assigns corpus audio files to MusicBrainz release track slots via a 4-stage process: candidate identification, directory-constrained scoring, tiered conflict resolution, and gap analysis. All packing is constrained to a single directory (or sibling directories for multi-medium releases).

Source: `src/meta/computations/analysis/release_packing.rs`

---

## Pipeline Overview

| Stage | Computation | Purpose |
|-------|------------|---------|
| 1 | PackReleases | Identify candidates, write manifest, spawn per-release scorers |
| 2 | ScoreReleaseCandidates (×N) | Directory selection, AcoustID scoring, elimination matching |
| 3a | ComputeReleaseMappings | Classify proposals into quality tiers |
| 3b–3e | Map{Perfect,FullMatch,Incomplete,Single}Releases → N×ResolvePackingComponent | Tiered orchestrators: find connected components, spawn parallel per-component MIS solvers. Knots extracted and resolved greedily by orchestrators. Isolated nodes emitted directly. (Incomplete/Single order configurable) |
| 4 | EmitUnmatchedSignals | Emit signals for unmatched corpus tracks and unfilled release slots |

Stages are barrier-separated: each defers the next via `SharedMappingState`, ensuring serial execution managed by the Witch's computation scheduler.

---

## Stage 1: PackReleases (Orchestrator)

**Trigger:** Manual only (operator requests release packing).

1. Load all AcoustID external matches for corpus files
2. Filter recordings by `min_confidence` (from config). Duration is not filtered — it is a scoring dimension in Stage 2.
3. Parse release tracklists from MB cache (locale-resolved artist names)
4. Write manifest rows (`release_id`, `total_tracks`, `title`, `artist`) to `packing_manifest` table
5. Deduplicate candidates per `(release_id, inode)` — keep highest-confidence recording
6. Write candidate rows to `release_packing_candidates` intermediate table
7. Spawn N `ScoreReleaseCandidates` computations (one per release with candidates)
8. Defer `ComputeReleaseMappings` via barrier

**Key data:**
- `CorpusFileInfo`: `parent_dir`, tags (`TITLE`/`ARTIST`/`ALBUM`/`TRACKNUMBER`), `duration_ms`
- `dir_file_count`: total audio files per directory (written to candidates table)

---

## Stage 2: ScoreReleaseCandidates (Parallel)

Each instance handles one release. Four phases execute sequentially.

### Phase 1: Exhaustive Per-Directory Scoring

All packing is constrained to target directory(ies) selected by `score_all_directories()`. This runs Hungarian assignment for every candidate directory and picks the one producing the highest total assignment score — replacing the former heuristic-based `select_target_directory()` that could produce non-deterministic results.

**Algorithm:**
1. Collect all unique directories containing candidate inodes
2. **Multi-medium** (`media.len() > 1`): try `find_sibling_dir_mapping()` to detect sibling directories (same parent) with candidates for different media. If found, filter candidates to the sibling set and run Hungarian — record as a candidate result.
3. **Per-directory**: for each individual directory, filter candidates to that directory, run Hungarian, compute total assignment score.
4. Select the `(TargetDirs, optimal_pairs)` with:
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

Both weight sets are configurable under `release-packing` in `config.kdl`.

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

### 3b–3e: MIS Rounds

Each round selects non-conflicting (inode-disjoint) proposals to maximize corpus coverage.

**Round flow (FullMatch, Incomplete):**
1. **Cull:** Discard proposals that lost any inode to prior rounds
2. **Dedup:** Group by sorted inode signature, keep best-scorer per group
3. **Knot extraction:** Build conflict graph (proposals sharing inodes are adjacent). Connected components with `proposals/inodes >= knot_ratio` (default 3.0) or `size > knot_size_limit` (default 50) are resolved greedily (best score first, skip conflicting)
4. **MIS solve:** Remaining clean components enter exact MIS:
   - Components ≤25 proposals: exhaustive bitmask enumeration (2^k subsets)
   - Components >25: branch-and-bound with coverage/score objective

**Perfect round** skips cull/dedup/knots — proposals are strict (all inodes must be unclaimed).

**Singles round:** per-inode best score, no MIS needed (no multi-inode conflicts).

**Round ordering** after FullMatch is configurable via `singles-before-incompletes`:
- `false` (default): Incomplete → Singles
- `true`: Singles → Incomplete

When singles run first, single-track releases claim inodes before the expensive Incomplete MIS round, preventing single-file incompletes from competing there.

Each tier orchestrator finds connected components in the conflict graph, then: (1) isolated nodes (size 1) are emitted directly, (2) knots are extracted and resolved greedily with signals emitted by the orchestrator, (3) clean multi-node components are spawned as parallel `ResolvePackingComponent` computations that independently build local conflict graphs and solve MIS. Inter-tier inode tracking reads assigned inodes from the DB (`signal_release_packing` table). AcoustID submissions for elimination winners are recorded per-component.

All packing signal tables (`signal_packed_release`, `signal_release_packing`, `signal_packing_knot`) are bulk-cleared at pipeline start (ComputeReleaseMappings, Stage 3a).

### PackedRelease (Categories)

| PackedReleaseCategory | Key Prefix | Criteria |
|-----------------------|------------|----------|
| Perfect | `perfect:` | All slots filled, all via AcoustID |
| FullMatch | `full_match:` | All slots filled, at least one via elimination |
| Single | `single:` | Single-track release |
| Incomplete | `incomplete:` | Partial coverage |

---

## Stage 4: EmitUnmatchedSignals

Post-resolution analysis producing 2 signal types:

### UnmatchedCorpusTrack
Emitted for inodes with AcoustID recording matches but no release assignment. Also covers fingerprinted files with no AcoustID match at all.

### UnfilledReleaseSlot
Empty track slots in partially-assigned releases. Only emitted for releases with `filled_count > 0`.

**Coverage filtering:** Suppresses `UnfilledReleaseSlot` signals for releases where every candidate inode is already assigned to a full-match release (cached MB entries, not real gaps).

---

## ProposalTier vs PackedReleaseCategory

These are distinct classification systems used at different stages:

- **ProposalTier** (Stage 3a): Input classification determining which MIS round pool a proposal enters. Based on slot coverage, directory structure, and leftover file counts.

- **PackedReleaseCategory** (Stage 3b-3e): Output classification for the signal, derived directly from `ProposalTier` (1:1 mapping: Perfect→Perfect, FullMatch→FullMatch, Incomplete→Incomplete, Single→Single). Emitted by ResolvePackingComponent (for multi-node components) or directly by tier orchestrators (for isolated nodes and knot winners). This is what the UI displays.

---

## Key Constants

| Constant | Value | Purpose |
|----------|-------|---------|
| `TITLE_PREASSIGN_THRESHOLD` | 0.95 | Title pre-assignment: high confidence, 1:1 only |
| ~~`ELIMINATION_SCORE_THRESHOLD`~~ | Removed | No threshold — directory constraint provides the quality gate |
| `DEFAULT_KNOT_RATIO` | 3.0 | Knot extraction: proposals/inodes threshold |
| `DEFAULT_KNOT_SIZE_LIMIT` | 50 | Max component size before forced knot extraction |

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
