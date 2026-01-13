# MLA Health Signals

Health signals are conditions detected in the corpus that may require attention or resolution. This document describes each signal type, how it's detected, and how it's resolved.

---

## Signal Lifecycle

```
┌───────────┐     ┌───────────┐     ┌───────────┐     ┌───────────┐
│ Detection │────▶│  Storage  │────▶│ Resolution│────▶│  Archive  │
└───────────┘     └───────────┘     └───────────┘     └───────────┘
   Scan/           health_          User flow          resolved_at
   Heartbeat       issues           or auto            + resolution_type
```

1. **Detection**: Signal identified during scan, heartbeat, or mutation
2. **Storage**: Recorded in `health_issues` with affected tracks
3. **Resolution**: User reviews and resolves via flow UI, or auto-resolved
4. **Archive**: Marked resolved with resolution type and session

---

## Signal Types

### FingerprintDuplicate

**Severity**: AutoResolvable or ManualReview

**Description**: Multiple files contain the same audio content (identical fingerprint).

**Detection Point**: During scan when fingerprint matches existing track.

**Key Format**: The shared fingerprint hash.

**Metadata**:
```json
{
  "track_ids": [123, 456, 789],
  "fingerprint": "AQADtM..."
}
```

**Resolution Options**:
- **Keep Best Quality**: Auto-resolve to highest bitrate/sample rate
- **Keep Specific**: User selects which copy to keep
- **Mark as Variants**: Accept as legitimate re-releases

**Resolution Flow**: `dedup_flow` cluster dialogue

---

### MetadataDuplicate

**Severity**: ManualReview

**Description**: Multiple files have identical artist/album/title but different fingerprints.

**Detection Point**: During scan when metadata matches but fingerprint differs.

**Key Format**: `artist|album|title`

**Metadata**:
```json
{
  "artist": "Artist Name",
  "album": "Album Name",
  "title": "Track Title",
  "track_ids": [123, 456]
}
```

**Resolution Options**:
- **Same Recording**: Mark one as canonical, other as variant
- **Different Recordings**: Accept both as legitimate (live vs studio, etc.)
- **Merge Metadata**: Combine best metadata from both

**Resolution Flow**: `dedup_flow` with metadata comparison view

---

### TagCanonical

**Severity**: Informational

**Description**: Tag values that could be unified (spelling variants, capitalization).

**Detection Point**: During scan via similarity detection, or during canon flows.

**Key Format**: `tag_name:canonical:variant` (e.g., `artist:The Beatles:Beatles, The`)

**Metadata**:
```json
{
  "tag_name": "artist",
  "canonical_value": "The Beatles",
  "variant_value": "Beatles, The",
  "track_count": 47,
  "confidence": 0.95
}
```

**Resolution Options**:
- **Accept Canonicalization**: Apply canonical form to all variants
- **Reject**: Keep variants as-is (different artists with similar names)
- **Choose Alternative**: Pick different canonical form

**Resolution Flow**: `canon_flow` for artists, `album_flow` for albums

---

### MissingTag

**Severity**: Informational

**Description**: Required tags are missing (currently: album_artist).

**Detection Point**: During scan or when entering album_artist flow.

**Key Format**: `missing_album_artist:album_name`

**Metadata**:
```json
{
  "album": "Album Name",
  "artists": ["Artist A", "Artist B"],
  "track_count": 12
}
```

**Resolution Options**:
- **Populate from Artist**: Copy artist to album_artist
- **Set Various Artists**: Mark as compilation
- **Manual Entry**: User provides value

**Resolution Flow**: `album_artist_flow` population phase

---

### QualityVariant

**Severity**: AutoResolvable

**Description**: Same audio content exists at different quality levels.

**Detection Point**: During duplicate detection when fingerprints match but quality differs.

**Key Format**: `quality:fingerprint_prefix`

**Metadata**:
```json
{
  "fingerprint": "AQADtM...",
  "variants": [
    {"track_id": 123, "bitrate_kbps": 320, "format": "mp3"},
    {"track_id": 456, "bitrate_kbps": 256, "format": "mp3"}
  ]
}
```

**Resolution Options**:
- **Keep Highest Quality**: Auto-select best (default)
- **Keep Specific**: User override
- **Keep All**: Accept quality variants

**Resolution Flow**: Auto-resolved during `dedup_flow` bulk phase

---

### DeployConflict

**Severity**: ManualReview

**Description**: Multiple corpus files would deploy to the same library path.

**Detection Point**: During deployment preview or health check.

**Key Format**: `deploy:library_name:target_path`

**Metadata**:
```json
{
  "library": "main",
  "target_path": "/library/Artist/Album/01 Track.mp3",
  "conflicting_paths": [
    "/corpus/web/Artist/Album/01 Track.mp3",
    "/corpus/cd/Artist/Album/01 Track.mp3"
  ]
}
```

**Resolution Options**:
- **Choose Source**: Select which corpus file to deploy
- **Rename**: Modify deployment path for one
- **Skip**: Don't deploy conflicting files

**Resolution Flow**: `deploy_flow` conflict resolution

---

### OutOfBandTagChange

**Severity**: ManualReview

**Description**: On-disk tags differ from indexed database values.

**Detection Point**:
- At heartbeat when `tag_mismatches` table has entries
- After external tools modify tags

**Key Format**: `oob_tag:track_id`

**Metadata**:
```json
{
  "track_id": 123,
  "field_count": 3,
  "fields": [
    {"field": "artist", "db_value": "Artist", "disk_value": "The Artist"},
    {"field": "album", "db_value": "Album", "disk_value": "Album (Remaster)"}
  ]
}
```

**Resolution Options**:
- **Accept Disk**: Update index to match on-disk values
- **Flush to Disk**: Write index values to file tags
- **Review Each**: Per-field decision

**Resolution Flow**: Future tag mismatch flow (not yet implemented)

---

### MissingFromIndex

**Severity**: Informational

**Description**: Audio files exist on disk but are not in the database index.

**Detection Point**: At heartbeat during corpus walk.

**Key Format**: `missing_from_index:directory_path`

**Metadata**:
```json
{
  "directory": "/corpus/new/artist/album",
  "file_count": 12,
  "sample_files": ["01 Track.mp3", "02 Track.mp3"]
}
```

**Resolution Options**:
- **Scan Directory**: Run incremental scan on affected paths
- **Ignore**: Files may be intentionally excluded

**Resolution Flow**: Trigger rescan from menu

---

## Detection Functions

Health signals are detected by functions in `corpus/health/detection.rs`:

| Function | Signals Detected |
|----------|-----------------|
| `detect_fingerprint_issues()` | FingerprintDuplicate, QualityVariant |
| `detect_metadata_issues()` | MetadataDuplicate |
| `detect_and_store_canonicalizations()` | TagCanonical |
| `detect_out_of_band_tag_changes()` | OutOfBandTagChange |
| `create_missing_from_index_issues()` | MissingFromIndex (in heartbeat) |

---

## Severity Levels

| Severity | Meaning | UI Treatment |
|----------|---------|--------------|
| `AutoResolvable` | Can be resolved automatically | Green, auto-processed in bulk phase |
| `ManualReview` | Requires user decision | Yellow, presented in review flow |
| `Informational` | Advisory only | Blue, shown in reports |

---

## Resolution Types

When an issue is resolved, the `resolution_type` field records how:

| Type | Meaning |
|------|---------|
| `merged` | Tracks were merged (one removed) |
| `kept_all` | All variants intentionally kept |
| `canonicalized` | Tag values unified |
| `populated` | Missing values filled |
| `accepted_disk` | On-disk values accepted |
| `flushed_to_disk` | Index values written to files |
| `scanned` | Files were indexed |
| `ignored` | Explicitly ignored |

---

## Health Invariants

The health system maintains these invariants:

1. **Unique Issues**: Each `(issue_type, issue_key)` pair is unique
2. **Track Linkage**: All affected tracks are linked via `health_issue_tracks`
3. **Cascade Deletion**: When tracks are deleted, their issue memberships are removed
4. **Resolution Finality**: Once resolved, issues are not re-opened (new issues created instead)

---

## Heartbeat Health Check

At startup, a quick health check runs:

```rust
HeartbeatResult {
    indexed_count: usize,        // Files in database
    disk_count: usize,           // Files on disk
    missing_from_disk: usize,    // In DB but not on disk
    new_on_disk: usize,          // On disk but not in DB
    pending_tag_flushes: usize,  // Mismatches pending flush
    library_health: Vec<...>,    // Per-library status
    duration: Duration,
}
```

Heartbeat creates `MissingFromIndex` health issues for new files.

---

## Reporting

Health summaries are available via:

1. **Heartbeat Display**: Shown in UI after startup check
2. **Health Reports**: Generated via Reports menu
3. **Tag Cloud**: Aggregated tag statistics with variant detection
4. **Database Queries**: Direct SQL access for custom analysis

See [DATABASE.md](DATABASE.md) for query examples.
