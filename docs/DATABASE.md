# MLA Database Schema

This document describes the SQLite database schema used by MLA. The database is stored at `$XDG_DATA_HOME/mla/mla.db`.

---

## Overview

The database is organized into functional groups:

| Group | Tables | Purpose |
|-------|--------|---------|
| **Core** | tracks, scan_state, scan_history | Audio file metadata and indexing |
| **Changes** | pending_changes, change_sessions, tag_edit_history | Algebraic change tracking |
| **Health** | health_issues, health_issue_tracks, known_variants | Corpus health signals |
| **Canonicalization** | tag_canonicalization, tag_mismatches | Tag normalization |
| **Deployment** | deployment_log | Library hard-link tracking |
| **Legacy** | duplicate_groups, duplicate_group_members | Deprecated duplicate tracking |
| **Meta** | app_metadata, corpus_health_stats | Application state |

---

## Core Tables

### tracks

Primary table storing metadata for all indexed audio files.

```sql
CREATE TABLE tracks (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL UNIQUE,       -- Absolute path to file
    source TEXT NOT NULL,            -- 'corpus' or library name
    inode INTEGER NOT NULL,          -- Filesystem inode for identity
    file_size INTEGER NOT NULL,      -- Size in bytes
    file_type TEXT NOT NULL,         -- Extension (mp3, flac, etc.)

    -- Metadata tags
    artist TEXT,
    album TEXT,
    album_artist TEXT,
    title TEXT,
    track_number INTEGER,
    genre TEXT,                      -- Added via migration

    -- Audio properties
    duration_ms INTEGER,
    bitrate_kbps INTEGER,
    sample_rate INTEGER,

    -- Fingerprinting
    fingerprint TEXT,                -- Chromaprint fingerprint
    isrc TEXT,                       -- International Standard Recording Code

    scanned_at DATETIME DEFAULT CURRENT_TIMESTAMP
);
```

**Indexes**: source, inode, artist, album, album_artist, title, duration_ms, fingerprint

**Source Values**:
- `corpus` - Files in the main corpus directory
- `<library_name>` - Files in a library directory (e.g., "main", "djay")

### scan_state

Tracks file state for incremental scanning optimization.

```sql
CREATE TABLE scan_state (
    id INTEGER PRIMARY KEY,
    source TEXT NOT NULL,            -- 'corpus' or library name
    inode INTEGER NOT NULL,          -- Filesystem inode
    path TEXT NOT NULL,              -- Absolute path at scan time
    mtime_secs INTEGER NOT NULL,     -- Modification time (seconds)
    mtime_nanos INTEGER NOT NULL,    -- Modification time (nanoseconds)
    file_size INTEGER NOT NULL,
    scanned_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(source, inode)
);
```

**Usage**: During incremental scans, files are checked against this table. If inode exists with matching mtime, the file is skipped. Changed mtime triggers re-scan.

### scan_history

Log of completed scans.

```sql
CREATE TABLE scan_history (
    id INTEGER PRIMARY KEY,
    source TEXT NOT NULL,
    file_count INTEGER NOT NULL,
    started_at DATETIME NOT NULL,
    completed_at DATETIME NOT NULL
);
```

---

## Change Tracking Tables

MLA uses algebraic change tracking where mutations are recorded as composable operations.

### pending_changes

Individual change operations awaiting execution.

```sql
CREATE TABLE pending_changes (
    id INTEGER PRIMARY KEY,
    session_id TEXT NOT NULL,        -- Links to change_sessions
    change_type TEXT NOT NULL,       -- See ChangeType enum
    source_path TEXT NOT NULL,       -- File being modified
    target_path TEXT,                -- For moves/copies
    metadata_changes TEXT,           -- JSON for tag edits
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    status TEXT DEFAULT 'pending'    -- pending, staged, committed, reverted
);
```

**Change Types**:
- `TagEdit` - Modify metadata tags
- `Move` - Move file to new location
- `Delete` - Remove file from corpus
- `Deploy` - Create hard link in library
- `Undeploy` - Remove hard link from library
- `Redeploy` - Update deployment (remove + create)

### change_sessions

Groups related changes into atomic units.

```sql
CREATE TABLE change_sessions (
    id INTEGER PRIMARY KEY,
    session_id TEXT NOT NULL UNIQUE, -- UUID
    description TEXT,                -- Human-readable description
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    committed_at DATETIME,           -- NULL until committed
    status TEXT DEFAULT 'active'     -- active, committed, abandoned
);
```

### tag_edit_history

Audit trail of tag modifications for potential rollback.

```sql
CREATE TABLE tag_edit_history (
    id INTEGER PRIMARY KEY,
    track_id INTEGER NOT NULL REFERENCES tracks(id),
    field_name TEXT NOT NULL,        -- artist, album, title, etc.
    old_value TEXT,
    new_value TEXT,
    edited_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    session_id TEXT                  -- Links to change session
);
```

---

## Health Tables

### health_issues

Detected health problems in the corpus.

```sql
CREATE TABLE health_issues (
    id INTEGER PRIMARY KEY,
    issue_type TEXT NOT NULL,        -- See HealthIssueType
    issue_key TEXT NOT NULL,         -- Unique identifier within type
    severity TEXT NOT NULL,          -- auto_resolvable, manual_review, informational
    discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    resolved_at DATETIME,            -- NULL if unresolved
    resolution_type TEXT,            -- How it was resolved
    resolution_session TEXT,         -- Session that resolved it
    metadata_json TEXT               -- Type-specific details
);
```

**Issue Types**:

| Type | Key Format | Description |
|------|------------|-------------|
| `fingerprint_dup` | fingerprint hash | Same audio content |
| `metadata_dup` | artist\|album\|title | Same metadata, different audio |
| `tag_canon` | tag:canonical:variant | Tag spelling variant |
| `missing_tag` | missing_X:album | Required tag missing |
| `quality` | quality:key | Same content, different quality |
| `deploy_conflict` | path collision | Would overwrite in library |
| `oob_tag` | oob_tag:track_id | On-disk differs from index |
| `missing_from_index` | missing_from_index:dir | File not yet indexed |

### health_issue_tracks

Links tracks to health issues (many-to-many).

```sql
CREATE TABLE health_issue_tracks (
    id INTEGER PRIMARY KEY,
    issue_id INTEGER NOT NULL REFERENCES health_issues(id) ON DELETE CASCADE,
    track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    role TEXT NOT NULL               -- 'member', 'canonical', 'variant', 'keep', 'remove'
);
```

### known_variants

Accepted duplicates that should not trigger health issues.

```sql
CREATE TABLE known_variants (
    id INTEGER PRIMARY KEY,
    variant_type TEXT NOT NULL,      -- 'rerelease', 'remix', 'remaster', etc.
    canonical_fingerprint TEXT NOT NULL,
    variant_fingerprint TEXT,
    canonical_track_id INTEGER REFERENCES tracks(id) ON DELETE SET NULL,
    variant_track_id INTEGER REFERENCES tracks(id) ON DELETE SET NULL,
    marked_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    notes TEXT
);
```

---

## Canonicalization Tables

### tag_canonicalization

Mappings from variant tag values to canonical forms.

```sql
CREATE TABLE tag_canonicalization (
    id INTEGER PRIMARY KEY,
    tag_name TEXT NOT NULL,          -- 'artist', 'album_artist', 'genre', 'album'
    canonical_value TEXT NOT NULL,   -- The preferred form
    variant_value TEXT NOT NULL,     -- The variant to replace
    confidence REAL,                 -- Detection confidence (0.0-1.0)
    auto_detected INTEGER DEFAULT 1, -- 1 if machine-detected
    confirmed_at DATETIME,           -- NULL until user confirms
    UNIQUE(tag_name, variant_value)
);
```

**Example Records**:
```
| tag_name | canonical_value | variant_value |
|----------|-----------------|---------------|
| artist   | The Beatles     | Beatles       |
| artist   | The Beatles     | Beatles, The  |
| genre    | Electronic      | Electronica   |
```

### tag_mismatches

Tracks where database values differ from on-disk tags.

```sql
CREATE TABLE tag_mismatches (
    id INTEGER PRIMARY KEY,
    track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    field TEXT NOT NULL,             -- Tag field name
    db_value TEXT,                   -- Value in database
    disk_value TEXT,                 -- Value on disk
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(track_id, field)
);
```

**Usage**: After canonicalization updates the database, mismatches are recorded until tags are flushed to disk. Also detects out-of-band changes (external tag edits).

---

## Deployment Tables

### deployment_log

Record of corpus files deployed to libraries.

```sql
CREATE TABLE deployment_log (
    id INTEGER PRIMARY KEY,
    library_name TEXT NOT NULL,      -- Target library
    corpus_path TEXT NOT NULL,       -- Source file in corpus
    deployed_path TEXT NOT NULL,     -- Destination in library
    inode INTEGER NOT NULL,          -- Shared inode (hard link)
    deployed_at DATETIME DEFAULT CURRENT_TIMESTAMP
);
```

---

## Application Tables

### app_metadata

Key-value store for application state.

```sql
CREATE TABLE app_metadata (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);
```

**Known Keys**:
- `health_data_version` - Version of health computation algorithm
- `last_scan_date` - ISO timestamp of last full scan

### corpus_health_stats

Cached corpus statistics.

```sql
CREATE TABLE corpus_health_stats (
    id INTEGER PRIMARY KEY,
    stat_type TEXT NOT NULL,
    last_updated DATETIME DEFAULT CURRENT_TIMESTAMP,
    data_json TEXT NOT NULL
);
```

---

## Migrations

The database supports automatic migrations for schema evolution:

1. **fingerprint** - Adds fingerprint column to tracks
2. **isrc** - Adds ISRC column to tracks
3. **genre** - Adds genre column to tracks
4. **unified_tag_canonicalization** - Migrates from separate artist/genre tables

Migrations are idempotent and run on every database open.

---

## Query Patterns

### Finding Duplicates by Fingerprint

```sql
SELECT fingerprint, COUNT(*) as count
FROM tracks
WHERE fingerprint IS NOT NULL
GROUP BY fingerprint
HAVING count > 1
ORDER BY count DESC;
```

### Finding Unresolved Health Issues

```sql
SELECT h.*, GROUP_CONCAT(t.path, ', ') as affected_files
FROM health_issues h
JOIN health_issue_tracks ht ON h.id = ht.issue_id
JOIN tracks t ON ht.track_id = t.id
WHERE h.resolved_at IS NULL
GROUP BY h.id;
```

### Finding Pending Tag Flushes

```sql
SELECT t.path, m.field, m.db_value, m.disk_value
FROM tag_mismatches m
JOIN tracks t ON m.track_id = t.id;
```

### Album Artist Population Candidates

```sql
SELECT album, artist, COUNT(*) as track_count
FROM tracks
WHERE album_artist IS NULL OR album_artist = ''
GROUP BY album, artist
ORDER BY track_count DESC;
```
