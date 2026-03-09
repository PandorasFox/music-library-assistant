# Release Packing → Tag Embedding Roadmap

Status: packing pipeline complete, tag embedding not started.

The hard algorithmic work — Hungarian assignment, MIS conflict resolution,
tiered proposal classification, knot extraction — is done and producing good
results. What follows is mostly plumbing: config knobs, operator review UX,
and wiring packing results into the existing tag write infrastructure.

## Where We Are

The release packing pipeline (Stages 1–4) is complete:

1. **PackReleases** — loads AcoustID matches, writes manifest, spawns scorers
2. **ScoreReleaseCandidates** — per-release Hungarian + elimination matching
   with 7 configurable scoring dimensions and two-phase title pre-assignment
3. **ComputeReleaseMappings → MIS rounds** — tiered conflict resolution:
   Perfect → FullMatch → Incomplete → Single, with knot extraction
   for dense components. Per-component signal emission within each tier.
4. **EmitUnmatchedSignals** — signal emission for unmatched corpus tracks
   and unfilled release slots

Results are browseable in the release packing browser (read-only).

## What's Next

### 0. ~~Splitting up Perfect and Full Match in UI, Coalescing NearMiss and Incomplete~~

Done. NearMiss has been removed as a signal type (folded into Incomplete). Perfect and FullMatch are now distinct ProposalTier/PackedReleaseCategory values with 1:1 mapping.

### 1. Config Knobs & Knot Exposure

**Knot stage toggles.** Knot extraction currently runs on FullMatch and
Incomplete MIS rounds. Perfect matches are dense enough that knots rarely
form, but the option should exist per-tier. Simple bool toggles in config:

```kdl
release-packing {
    knot-extraction {
        perfect false       // rarely needed
        full-match true     // default
        incomplete true
    }
}
```

**Knot review UI.** Extracted knots are currently resolved by best-scorer and
logged. These are the cases where N releases compete for a small set of files
and the MIS solver can't efficiently untangle them. We need a UI to present
knots for operator review — show the competing releases, overlapping inodes,
and let the operator pin/reject specific proposals.

### 2. Tag Embedding Config

How to serialize matched release metadata into tags. The existing
`tag_templates` config field is parsed but unused — this is where it plugs in.

Key decisions that need config surface:

- **Artist field composition.** MB has structured artist credits with
  join phrases ("feat.", "&", "×"). **This is undesirable** because it does not
  conform to Vorbis Comment tag structure. We want to transform the artist
  credits into individual tags per artist.
    - Config controls:
      - prefer locale-specific names (ja → original script),
        - `preferred_locales` already exists in config.
      - include/exclude featured vocalists in track titles
      - album artist?

- **Tag field mapping.** Which MB fields map to which tag names. Defaults
  should cover standard Vorbis comments and ID3v2 frames. The `tag_templates`
  config field was designed for this. Musicbrainz is pretty id3v2-oriented,
  and we want to prioritize Vorbis comments. id3v2 adapter can be done after.

- **Multi-value strategy.** Vorbis supports multiple values per field natively.
  ID3v2 uses null-separated values. Config option for how to handle
  multi-artist, multi-genre, etc.

- **What to write.** Not everything from MB should be written. Config for
  which fields to emit (title, artist, album, albumartist, tracknumber,
  discnumber, date, musicbrainz_* IDs, etc.).

### 3. Release Acceptance & Tag Application

The bridge between "packing computed good results" and "tags written to files."

**Acceptance flow:**

1. Operator reviews a packed release in the browser
2. Operator confirms ("accept this release match")
3. System generates tag edit operations from the release assignment
4. Operations enter the normal decision/transaction pipeline
5. Tags get written to DB then flushed to disk via existing `ApplyTagOpsMutation`

This requires:

- **New `DecisionKey` variant** for release acceptance (keyed by release_id)
- **Tag generation function** that takes a release assignment (inode → track
  slot mapping) and produces tag edit operations using the embedding config
- **Bulk acceptance** — accept all Perfect matches without individual review,
  review FullMatch individually
- **MB cache integration** — the release JSON is already cached; tag
  generation reads from cache, no network needed

### 4. Release Lifecycle: Solved State & Cache Management

Once tags are applied and verified, the release is "solved" — its corpus
files have authoritative metadata, and the cached MB data + intermediate
packing tables can be cleaned up.

- **Solved marking.** A release moves from "packed" to "solved" after tag
  application succeeds and the operator confirms. Solved releases are excluded
  from future packing runs.
- **Cache eviction.** MB release JSON cache can be pruned for solved releases
  (or retained with a long TTL for re-verification).
- **Re-solve triggers.** If corpus files change (re-rip, new files added to
  directory), the solved state should be invalidated and the release
  re-entered into the packing pipeline.

## Non-Goals (For Now)

- Multi-source matching (Discogs, etc.) — the `ExternalSource` enum is ready
  but this is a separate effort
- Cover art embedding — image pipeline exists but is orthogonal. Cover art sourcing requires a new external source.
- ID3/MP3 serialization options - we'll need better comprehension of mp3 tags at the 'what id3 level are we at' (v1, v2.3, v2.4)
  - global config opinion?
  - needs investigation into what Lofty supports here. null separator? repeated tag frames?

### Non-Goals (Always and forever)
- Automated acceptance without operator review — the whole point is
  Spotify-level data quality through batch reasoning and mild operator review.
