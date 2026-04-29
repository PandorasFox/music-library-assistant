# Signal Reference

> **Maintenance Requirement**: Adding, removing, or changing signal semantics MUST be
> reflected in this document. Each signal type must have documented emitters and clearers.

## Source Location

Signal types are defined in `src/meta/signals/types.rs`:
- `SignalType` - top-level enum (`CorpusFile(CorpusFileSignalType)`, `Aggregate(AggregateSignalType)`)
- `CorpusFileSignalType` - per-file signal variants
- `AggregateSignalType` - aggregate signal variants
- `Signal`, `AggregateSignal` - signal data structs
- `SignalSummary`, `CorpusSummary` - summary types for UI display

## Overview

Signals are atomic facts about corpus state. They follow these principles:
- **Individual**: One signal per file/track/issue (no aggregate counts)
- **Idempotent**: Creating an existing signal is a no-op
- **Witnessed**: All signal operations require a witness token

---

## Corpus File Signals (per-file)

| Signal | Emitted By | Cleared By | Meaning |
|--------|------------|------------|---------|
| FileInCorpus | ScanCorpusDirectory | DeriveCorpusSignals (stale reconciliation) | File discovered on disk |
| UnindexedFile | DeriveDirectorySignals | DeriveDirectorySignals, mutations | On disk but not in index |
| MissingFile | DeriveDirectorySignals | DeriveDirectorySignals, mutations | In index but not on disk |
| MissingDirectory | ScheduleSecondLevelDerivations | ScheduleSecondLevelDerivations, DropDirectoryFromIndex | Indexed directory no longer on disk |
| HealthyFile | DeriveDirectorySignals | DeriveDirectorySignals, mutations | In corpus, indexed, mtime matches, no OOB signals |
| CorruptFile | VerifyTags, VerifyAudio, Transcode | VerifyAudio (if valid), MoveToStash, DropFromIndex | Tag read or audio decode failed |
| LosslessRemux | IndexFileFromPath, DetectLosslessRemux | Transcode (to FLAC), DetectLosslessRemux | Lossless non-Vorbis container (WAV, AIFF, APE, WV) |
| SubparDuplicate | AnalyzeFingerprintOverlaps | AnalyzeFingerprintOverlaps, MoveToStash | Track is outranked by a better version in its duplicate group (SubparFormat, SubparBitrate, or SubparSampleRate). Never emitted for equivalent-tier ties |
| OutOfBandTagSync | VerifyTags | VerifyTags, resolution mutations | One-way tag difference (syncable) |
| OutOfBandTagConflict | VerifyTags | VerifyTags, resolution mutations | Two-way tag conflict |
| MtimeOnlyMismatch | VerifyTags | VerifyTags, AcknowledgeMtimeOnly | Mtime changed, tags identical |
| MovedFile | DeriveCorpusSignals | UpdateFilePath | Same inode at different path. Columns: `old_path`. Same-zone: disk path differs from indexed path in `both` set |
| InodeChanged | *(not currently emitted — replaced by watcher FileRemoved+FileCreated)* | AcknowledgeInodeChanged | File was replaced (same path, new inode) |
| ExpectedMissingTag | EmitExpectedMissingTag | — | Operator-confirmed expected missing tag (persistent suppression). Table: `signal_expected_missing_tag`. Suppresses MissingAlbumSingleSignal for this inode in DetectMissingTags |
| MusicBrainzTagged | DetectMusicBrainzTagged | DetectMusicBrainzTagged | File has both configured MB track + release tags present (fully matched to a MusicBrainz release). Table: `signal_musicbrainz_tagged` (inode PK). Files with this signal are elided from tag-based health checks (MissingTags, TagCanonicity, CompoundTag, InconsistentAlbumArtist, DiscExtraction, PathTagMismatch) |
| PathTagMismatch | DetectPathTagMismatches | DetectPathTagMismatches | File path doesn't match source dir's path-tag schema. Data (bincode BLOB): `source_dir`, `schema_template`, `mismatch_kind` (StructureMismatch or ValueMismatch with per-tag details). Table: `signal_path_tag_mismatch` (inode PK, path, data BLOB, data_hash) |
| ExternalMatch | DeriveExternalMatches | DeriveExternalMatches | AcoustID recording metadata compared against corpus tags. Data (bincode BLOB): `source`, `recording_id`, `confidence`, `classification` (ExactMatch/ContentDiff/MetadataOnly), `diffs[]` (per-tag differences), `total_candidates`, `release_id`, `release_group_id`. Table: `signal_external_match` (inode PK, path, data BLOB, data_hash) |
| ReleasePacking | ResolvePackingComponent + tier orchestrators (isolated nodes + knot winners) | Bulk-cleared at pipeline start (ComputeReleaseMappings) | Bin-packed release assignment for a corpus file. Data (bincode BLOB): `release_id`, `release_title`, `release_artist`, `track_position`, `medium_position`, `medium_format`, `track_number`, `recording_id`, `track_title`, `score` (composite 0.0-1.0), `score_breakdown` (acoustid_confidence, duration_match, title_match, artist_match, album_match, track_number_match), `alternatives_count` (local to component, not global), `release_coverage` (fraction of release tracks matched), `match_method` (`AcoustId` = matched via AcoustID fingerprint lookup in Stage 2 scoring, `Elimination` = matched by per-release elimination in Stage 2 when all AcoustID-assigned siblings map to same release). Table: `signal_release_packing` (inode PK, path, data BLOB, data_hash). Emitted per-component: isolated nodes and knot winners directly by orchestrators, multi-node clean components by parallel ResolvePackingComponent computations. See `docs/RELEASE_PACKING_ALGORITHM.md` for the full algorithm reference |
| UnmatchedCorpusTrack | EmitUnmatchedSignals (Stage 4) | EmitUnmatchedSignals | Corpus file with AcoustID recording matches but no release assignment after global resolution. Only emitted for inodes that were scored (had candidates) but lost during greedy assignment. Data (bincode BLOB): `recording_ids[]`, `considered_release_ids[]`. Table: `signal_unmatched_corpus_track` (inode PK, path, data BLOB, data_hash) |

---

## Database State Flags

These are not signals but database columns that track synchronization state.

| Flag | Set By | Cleared By | Meaning |
|------|--------|------------|---------|
| needs_disk_flush | ApplyTagOps | ApplyDbTagsToDisk | DB tags changed but not yet synced to disk file |

### Pending AcoustID Submissions

The `pending_acoustid_submissions` table records fingerprint-to-recording associations discovered by elimination matching that should be submitted to AcoustID to improve the public database.

| Column | Type | Meaning |
|--------|------|---------|
| fingerprint | TEXT (PK) | Chromaprint fingerprint of the corpus file |
| recording_id | TEXT (PK) | MusicBrainz recording MBID assigned by elimination |
| duration_ms | INTEGER | Audio duration in milliseconds |
| source | TEXT | How the association was discovered (default: `'elimination'`) |

These rows are written by ComputeReleaseMappings (Stage 3) for elimination-matched winners — corpus files assigned to release track slots via per-release elimination in Stage 2 rather than direct AcoustID lookup. The fingerprint + recording_id pair, once submitted, would allow future AcoustID lookups to find this match directly.

### Recovery via needs_disk_flush

Tracks with `needs_disk_flush = TRUE` can be recovered via the OOB modal:

```sql
SELECT * FROM tracks WHERE needs_disk_flush = 1;
```

For each track, re-queue an `ApplyDbTagsToDisk` mutation. Since `ApplyDbTagsToDisk` reads from the database (source of truth), it's idempotent and can be safely re-run.

---

## Library File Signals (per-file)

| Signal | Emitted By | Cleared By | Meaning |
|--------|------------|------------|---------|
| LibraryLeftover | DeriveDeployHealthSignals | UpdateDeploySignals, mutations | Library file with no corpus backing |
| LibraryStale | DeriveDeployHealthSignals | UpdateDeploySignals, LibraryMove | Library file at wrong path (audio or sidecar image) |
| DeployReady | DeriveCorpusDeployStatus | UpdateDeploySignals, HardLink | Healthy corpus file not deployed. Metadata: `{ "deploy_path": "..." }` |
| DeployedHealthy | DeriveCorpusDeployStatus, UpdateDeploySignals | DeriveCorpusDeployStatus | Healthy corpus file correctly deployed. Metadata: `{ "library_path": "{library_name}/..." }` |
| SidecarDeployReady | DeriveCorpusDeployStatus | DeriveCorpusDeployStatus (reconcile) | Corpus sidecar image not yet deployed to library. Tiebreak winner if conflicting. Keyed by image inode. BLOB data: role, format, width, height |

---

## Aggregate Signals

Aggregate signals group multiple tracks by a shared characteristic. They use set reconciliation for efficient recomputation.

| Signal | Emitted By | Cleared By | Meaning |
|--------|------------|------------|---------|
| SidecarDeployConflict | DeriveCorpusDeployStatus | DeriveCorpusDeployStatus (reconcile) | Multiple corpus images target the same library sidecar path. Key: "library_name/deploy_path". Data (BLOB): Vec of conflicting corpus image inodes. Tiebreak winner (alphabetically first corpus path) still gets SidecarDeployReady |
| FingerprintOverlap | DetectFingerprintOverlaps | DetectFingerprintOverlaps | Tracks with identical fingerprints (internal signal) |
| CrossSourceOverlap | DetectCrossSourceOverlaps | DetectCrossSourceOverlaps | Fingerprint overlaps spanning different source directories (from config `dir` stanzas). Key: sorted source pair, e.g., "web/releases/bandcamp\|web/releases/indie". Within-source overlaps are ignored. Metadata: `source_a`, `source_b`, `*_priority`, `*_can_stash`, `overlap_count`, `fingerprint_keys[]`, `track_pairs[]` |
| DuplicateInode | DetectDuplicateInodes | DetectDuplicateInodes | Tracks sharing same inode |
| MissingTag | DetectMissingTags | DetectMissingTags | Tracks missing required tags. Files with ALBUM missing but ARTIST+TITLE present (and not suppressed by ExpectedMissingTag) are routed to MissingAlbumSingleSignal instead |
| MissingAlbumSingleSignal | DetectMissingTags | DetectMissingTags | Tracks missing ALBUM tag but having ARTIST+TITLE. Key: lowercased artist name. Data (BLOB): artist name + tracks list. Suppressed for inodes with ExpectedMissingTag |
| MetadataDuplicate | DetectMetadataDuplicates | DetectMetadataDuplicates | Tracks with identical tag sets |
| TagCanonicity | Detect{Artist,AlbumArtist,Album,Genre}TagCanonicalizations | Detect{Artist,AlbumArtist,Album,Genre}TagCanonicalizations (each scoped to its own `tag_name` key prefix) | Similar tags needing unification. Key format: `"{tag_name}:{normalized_key}"` — the tag-name prefix scopes reconciliation so the four parallel sibling computations (`artist:`, `album_artist:`, `album:`, `genre:`) only clear within their own namespace |
| ArtistNeedsPlural | DetectCompoundTagsForInode | DetectCompoundTagsForInode (clears when condition resolves), ApplyTagOps | Multi-valued ARTIST/ALBUMARTIST without plural form (ARTISTS/ALBUMARTISTS). Data (bincode BLOB): `needs_artist`, `needs_album_artist`, `artist_values[]`, `album_artist_values[]`. Table: `signal_artist_needs_plural` (inode PK, path, data BLOB, data_hash). Resolution: semicolon-join singular, copy originals to plural tag |
| CompoundTag | DetectCompoundTagsForInode | DetectCompoundTagValues (clears all before spawning) | Per-file signal for tags matching a `SplitRule` in the priority chain. `separator` field contains either the literal separator string (e.g., `";"`) or a collaboration keyword label (e.g., `"feat."`, `"vs."`). Metadata: `{ inode, compounds: [{ tag_name, compound_value, split_parts, separator }] }` |
| CanonicalTag | EmitCanonicalTag | - | Operator-confirmed canonical tag value (whitelist). Key: `{tag_name}:{tag_value}`. Prevents compound detection from flagging this value. Also suppresses TagCanonicity collision groups where any variant has a CanonicalTag |
| ExpectedOverlap | EmitExpectedOverlap | - | Operator-confirmed expected source pair overlap (whitelist). Key: sorted `"source_a\|source_b"` pair. Suppresses CrossSourceOverlap signal emission for this pair in DetectCrossSourceOverlaps. Also clears any existing CrossSourceOverlap signal for the pair when emitted |
| ExpectedDuplicate | EmitExpectedDuplicate | - | Operator-confirmed expected fingerprint overlap (whitelist). Key: fingerprint text (same key space as RedundantDuplicate). Suppresses RedundantDuplicate and SubparDuplicate signal emission for this fingerprint group in AnalyzeFingerprintOverlaps. Also clears any existing RedundantDuplicate signal for the key when emitted |
| InconsistentAlbumArtist | DetectInconsistentAlbumArtist | DetectInconsistentAlbumArtist | Album with inconsistent artist. Suppressed when any track in the group has `COMPILATION=0` tag |
| RedundantDuplicate | AnalyzeFingerprintOverlaps | AnalyzeFingerprintOverlaps | Group of files with identical fingerprints and equivalent quality tier (same format class + metric). Requires operator choice — neither file is subpar. Key: fingerprint text. Data (bincode): `file_type`, `inodes[]`, `paths[]` |
| DiscExtraction | DetectDiscExtractions | DetectDiscExtractions | Disc number extractable from ALBUM or TRACKNUMBER tags. Two sources: **Album** — ALBUM tag matches `,?\s*disc\s+(\d+)\s*$` (e.g., "Album, Disc 2" → ALBUM="Album" + DISCNUMBER="2"); **TrackNumber** — TRACKNUMBER matches `^([A-Za-z]+)(\d+)$` prefix pattern (e.g., "A01" → TRACKNUMBER="01" + DISCNUMBER="A"). Key: `album:{cleaned_album_lower}\|{disc_number}` or `tracknum:{album_lower}\|{album_artist_lower}\|{prefix_lower}`. Data (bincode BLOB): `DiscExtractionData { source: DiscExtractionSource, inodes: Vec<i64> }` |
| UnfilledReleaseSlot | EmitUnmatchedSignals (Stage 4) | EmitUnmatchedSignals | Release track position with no matching corpus file after global resolution. Only emitted for releases with at least one filled slot. Key: `{release_id}:{medium_pos}:{track_pos}`. Data (bincode BLOB): `release_id`, `release_title`, `release_artist`, `medium_pos`, `track_pos`, `track_title`, `recording_id`, `filled_count`, `total_tracks`. Table: `signal_unfilled_release_slot` |
| PackedRelease | ResolvePackingComponent + tier orchestrators (isolated nodes) | Bulk-cleared at pipeline start (ComputeReleaseMappings) | Per-release aggregate packing result with typed category. Key: `{category_prefix}:{release_id}` where prefix is `perfect`, `full_match`, `single`, `incomplete`, or `low_confidence`. Data (bincode BLOB): `release_id`, `release_title`, `release_artist`, `category` (PackedReleaseCategory enum), `assigned_count`, `total_tracks`. Table: `signal_packed_release`. Categories: **Perfect** = all tracks matched via AcoustID; **FullMatch** = all tracks matched, some via elimination; **Single** = single-track release; **Incomplete** = some but not all tracks matched; **LowConfidence** = FullMatch/Incomplete downgraded because AcoustID ratio < `low-confidence-max-acoustid-ratio` AND avg album_match < `low-confidence-max-album-match` (likely mispack from elimination filling slots on wrong release). Directory-constrained packing ensures all assignments come from a single directory (or sibling directories for multi-medium), so cross-directory scattering is structurally impossible. Enables efficient per-category SQL counts via `WHERE key LIKE 'perfect:%'` etc. Emitted per-component: isolated nodes directly by orchestrators, multi-node components by parallel ResolvePackingComponent computations |
| PinnedReleaseConflict | ComputeReleaseMappings (Stage 3a) | Bulk-cleared at pipeline start (ComputeReleaseMappings) | Two or more source dirs both pin the same release, but the release has fewer media than pinning dirs — no valid assignment exists. Key: `release_id`. Data (bincode BLOB): `release_id`, `release_title`, `directories[]` (list of pinning dir paths), `reason` (string). Hard stop: neither dir is packed. Operator must resolve by adjusting `pinned_release` in `dirs.kdl`. Table: `signal_pinned_release_conflict`. UI: shown as a conflict count in the External Matches view with a staleness indicator |
| PackingKnot | Tier orchestrators (MapFullMatch/Incomplete/SingleReleases) | Bulk-cleared at pipeline start (ComputeReleaseMappings) | Dense conflict graph component (knot) extracted by tier orchestrators. Key: `{tier}:{knot_id}` (e.g., `full_match:3`). Data (bincode BLOB): `tier`, `knot_id`, `classification` (ByRatio or BySize), `ratio`, `contested_inodes[]`, `proposals[]` (each with `release_id`, `release_title`, `release_artist`, `total_tracks`, `total_score`, `selected` (greedy winner), `assignments[]` (each with `inode`, `recording_id`, `medium_pos`, `track_pos`, `track_title`, `score`, `score_breakdown`, `match_method`)). Table: `signal_packing_knot`. Knots are components where proposals/inodes >= knot_ratio (default 3.0) or component size > knot_size_limit (default 50). Resolved greedily by best score, emitted directly by tier orchestrators (not spawned as components). **UI:** External Matches → Release Packing → Knots → KnotBrowser (two-pane read-only view with Tab/Shift+Tab knot cycling) |
| AlternativeReleasePacking | ResolvePackingComponent + tier orchestrators (isolated nodes + knot discography winners) | Bulk-cleared at pipeline start (ComputeReleaseMappings) | Alternative release with identical inode signature (same corpus files) to a winning release. Different pressings/editions/regional variants that pack identically. Key: `{winner_release_id}:{alt_release_id}`. Data (bincode BLOB): `winner_release_id`, `winner_release_title`, `alternative_release_id`, `alternative_release_title`, `alternative_release_artist`, `alternative_score`, `winner_score`, `inode_count`. Table: `signal_alternative_release_packing`. Detected during dedup-by-inode-signature in partial tiers (FullMatch/Incomplete/Single) and via bookkeeping in Perfect tier. Scoped per individual winning release — only proposals with byte-identical sorted inode sets qualify |
| VariousArtistsOverride | ResolvePackingComponent + tier orchestrators (isolated nodes + knot discography winners) | Bulk-cleared at pipeline start (ComputeReleaseMappings) | Suggested non-VA artist for a winning release with "Various Artists" as album artist. Key: winner `release_id`. Data (bincode BLOB): `release_id`, `release_title`, `suggested_artist`, `source` (ExactAlternative or CompetingProposal). Table: `signal_various_artists_override`. Two-tier lookup: (1) most frequent non-VA artist from exact alternative siblings, (2) fallback to most frequent non-VA artist from competing proposals in the same component that overlap the winner's inodes |
| SameRecordingDifferentRelease | AnalyzeFingerprintOverlaps | AnalyzeFingerprintOverlaps (via reconciliation) | Same MusicBrainz recording on different releases. Key: MB recording MBID. Data (bincode BLOB): `recording_id`, `entries[]` (each with `inode`, `path`, `mb_release_id`, `mb_track_id`, `album`, `file_type`, `bitrate_kbps`, `sample_rate`, `duration_ms`). Emitted when two fingerprint-matching files share a MUSICBRAINZ_TRACKID but have different MUSICBRAINZ_ALBUMID values. Table: `signal_same_recording_different_release` |
| DeployConflict | DetectDeployConflicts | DetectDeployConflicts | Multiple tracks mapping to same library path |
| ReleaseOverlap | DetectReleaseOverlaps | DetectReleaseOverlaps | Cross-source releases targeting the same album directory. Key: album directory (e.g., "Artist/Album"). Data (bincode BLOB): `releases[]` (each with `source_dir`, `release_dir`, `can_stash`, `inodes[]`, `corpus_paths[]`), `file_count`. Only emitted for cross-source overlaps (2+ configured sources). **UI Resolution:** Insights view → "Release overlaps" entry → DirectoryClusterModal with per-directory Stash/EditTags options |

---

## Signal Lifecycle

### Creation
```
ensure_file_signal_if_missing(db, sender, signal_type, key, witness)
  └─ Checks DB first (freshness optimization)
  └─ Queues INSERT OR IGNORE if signal doesn't exist
```

### Clearing
```
clear_file_signal_if_present(db, sender, signal_type, key, witness)
  └─ Checks DB first (freshness optimization)
  └─ Queues DELETE if signal exists
```

### Aggregate & Corpus Reconciliation
```
reconcile_aggregate_signals(db, sender, signal_type, computed, witness)
reconcile_corpus_signals(db, sender, signal_type, computed, witness)
  └─ Compares computed vs. stored (via data_hash for BLOB types, inode existence for scalar)
  └─ Clears stale (in DB, not computed)
  └─ Creates new (computed, not in DB)
  └─ Updates changed (both, but hash differs)
  └─ Skips unchanged (both, same hash)
  └─ Returns (cleared, new, updated, unchanged) counts
```

---

## Signal Design Principles

From `CLAUDE.md`:

1. **Signals must be small and individual** - One file/track per signal
2. **No aggregate signals for counts** - Compute counts via SQL at query time
3. **Signals are facts, not actions** - They describe state, not what to do
4. **Mutual exclusion where appropriate** - OOB signals (Sync/Conflict/MtimeOnly) are mutually exclusive

### GC Backstop

`DeriveCorpusSignals` includes a GC pass that clears orphaned corpus signals. After computing the known inode universe (disk inodes ∪ indexed inodes), it scans each corpus signal table for inodes outside that universe and deletes them. This catches signals that persist due to mutations that previously failed to return their affected inodes, or any future bugs in the post-mutation signal clearing pipeline.

Signal tables scanned: UnindexedFile, MissingFile, MovedFile, HealthyFile, CorruptFile, LosslessRemux, MtimeOnlyMismatch, OutOfBandTagSync, OutOfBandTagConflict, SubparDuplicate, CompoundTag, DeployReady, DeployedHealthy, SidecarDeployReady, MissingDirectory, ExternalMatch, ExpectedMissingTag, PathTagMismatch, ReleasePacking, UnmatchedCorpusTrack. FileInCorpus is excluded (it IS the disk observation). Note: UnfilledReleaseSlot is an aggregate signal (not inode-keyed) and is not subject to corpus GC backstop.

### Good Signals
- `UnindexedFile` for path X (one file)
- `FingerprintOverlap` for fingerprint Z (one group of tracks)
- `CrossSourceOverlap` for source pair (groups tracks by configured source directory)

### Bad Signals (DO NOT CREATE)
- `LibraryHealthSummary` (aggregate counts)
- Any "summary" signal that counts other signals

### Non-Signal Tracking: Sidecar Image Files

Sidecar cover images (cover.jpg, folder.png, etc.) are tracked via the `inode_paths` table (zone='corpus') joined with `inodes` (mtime, size) and `image_info` (format, width, height, role) rather than signals. Image metadata is populated by the `IndexImageFile` computation. Deployment of sidecar images alongside audio files is handled by `HardLink` mutation. This follows the principle that signals are for actionable corpus health facts, not for inventory tracking that is better served by direct table storage.
