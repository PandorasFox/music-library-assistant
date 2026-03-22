# Zone Architecture and FS Watcher

## Zone Model

MM uses two zones to distinguish file roles:

| Zone | Purpose |
|------|---------|
| `corpus` | Source-of-truth audio files, organized by source directories |
| `library` | Deployment targets (hardlinked from corpus) |

The `files` table contains files from both zones, distinguished by the `zone` column. Tags live in `corpus_tags`. Library files are deployment targets, not sources of truth.

### Zone Characteristics

```
Corpus: indexed audio files, tagged, canonical tag source, deployable, externally matchable
Library: deployment target only — not a source, not tagged, not matched
```

Key constraints:
1. **Tag canonicity is corpus-only.** `TagCanonicitySignal` and `InconsistentAlbumArtistSignal` are keyed by corpus tag state.
2. **Deploy path computation is corpus-only.** `DeriveCorpusDeployStatus` operates exclusively on corpus files.
3. **External matching is corpus-only.** AcoustID and MusicBrainz matching operate on corpus files.

## FS Watcher Thread

### Current Architecture (Polling-Based)

MM uses polling-based filesystem observation:

1. Startup: Witch queues `WalkCorpus` for the corpus zone
2. `ScanCorpusDirectory` does recursive `read_dir()` + per-file `metadata()` calls
3. Compares disk state against DB via mtime batch queries
4. Spawns `VerifyTags` + `VerifyAudio` for changed/new files
5. Idle rescan: configurable interval, re-walks corpus with mtime gating

This means:
- New files aren't detected until next poll
- Every poll re-walks the entire directory tree even if nothing changed
- The "initial FS state" is asserted by the same polling machinery, not a dedicated startup path

### Target Architecture (Event-Based)

Replace polling-based `WalkCorpus` -> `ScanCorpusDirectory` with a persistent filesystem watcher thread.

```
                    +---------------------+
                    |   FS Watcher Thread  |
                    |                      |
                    |  inotify/fanotify    |
                    |  per zone root       |
                    |                      |
                    |  Arc<RwLock<         |
                    |    ZoneState {       |
                    |      dirty: HashSet, |
                    |      initial_done,   |
                    |    }                 |
                    |  >>                  |
                    +----------+-----------+
                               | (writes)
                               v
                    +----------------------+
          (reads)   |  Shared State Handle |
     +--------------+                      |
     |              |  per-zone RwLock'd   |
     |              |  observed inode sets  |
     |              +----------------------+
     v
+------------+
|   Witch    |
|            |---- weaves watcher events into
|            |     computation pipeline
+------------+
```

### Startup Sequence (New)

1. DB migrations, config load, basic Witch setup
2. Spawn FS watcher thread, give it zone roots (corpus, library dirs)
3. Watcher does **initial enumeration** — walks each zone root, builds complete inode->path map, reports as initial state assertion
4. Witch receives initial state, runs derivation signals against it (same as today's post-walk derivation)
5. Watcher enters **steady state** — kernel events only, no polling
6. Witch reads dirty sets from watcher on each `tick()`, feeds into computation pipeline

### Watcher Thread State

```rust
pub struct FsWatcherHandle {
    /// Per-zone observed state. The watcher writes; the Witch reads.
    corpus: Arc<RwLock<ZoneWatchState>>,
    libraries: Arc<RwLock<ZoneWatchState>>,
    /// Control channel for shutdown, zone root changes, etc.
    control_tx: Sender<WatcherControl>,
}

struct ZoneWatchState {
    /// Inodes that have changed since last drain.
    /// The Witch drains this set on each tick.
    dirty_inodes: HashSet<i64>,
    /// Whether initial enumeration is complete for this zone.
    initial_scan_complete: bool,
    /// Full inode->path map (authoritative FS state).
    /// Updated by watcher in real-time.
    known_inodes: HashMap<i64, PathBuf>,
}
```

### Integration with Witch

The Witch's `tick()` currently drains `result_rx` for completed rayon tasks. With the FS watcher, it additionally:

```rust
fn tick(&mut self) {
    // ... existing rayon result drain ...

    // Drain FS watcher dirty sets
    if let Some(ref watcher) = self.fs_watcher {
        let corpus_dirty = watcher.drain_dirty(Zone::Corpus);

        if !corpus_dirty.is_empty() {
            self.queue_verify_for_inodes(Zone::Corpus, corpus_dirty);
        }
    }
}
```

This replaces the idle rescan timer entirely. Changes are detected in real-time via kernel events.

### What the Watcher Replaces

| Current | Watcher-Based |
|---------|---------------|
| `WalkCorpus` (full directory tree walk) | Initial enumeration (one-time), then kernel events |
| `ScanCorpusDirectory` (per-dir stat + read_dir) | No longer needed — watcher reports changes |
| `maybe_start_idle_rescan()` (timer) | Eliminated — changes are immediate |
| `VerifyMtime` as gating step | Watcher already knows what changed; skip mtime comparison |
| `gather()` stat checks | Watcher's `known_inodes` map is the truth |

### Library Considerations

Library zone is simpler — library files are deployment targets, not source material. The watcher monitors library roots for leftover detection (files appearing/disappearing that weren't placed by MM). This replaces `execute_walk_library()`.

### Fallback: Periodic Full Scan

Even with a watcher, a periodic full-scan backstop is wise (inotify can miss events under heavy load, after suspend/resume, etc.). This becomes a low-frequency (hourly?) consistency check rather than the primary observation mechanism.

### Platform Considerations

- **Linux:** `inotify` (per-directory watches) or `fanotify` (per-mount, requires `CAP_SYS_ADMIN`). inotify is simpler but requires a watch per directory; fanotify is more efficient for large trees.
- **Crate:** `notify` crate provides cross-platform abstraction. Reasonable starting point.
- **Watch limits:** `/proc/sys/fs/inotify/max_user_watches` may need increasing for large corpus trees. The watcher should detect `ENOSPC` and fall back to polling with a warning.
