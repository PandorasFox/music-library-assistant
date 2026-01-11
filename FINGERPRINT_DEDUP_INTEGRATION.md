# Fingerprint Deduplication Integration Guide

## Status

**Core Implementation: ✅ COMPLETE** (Phases 1-4)
**UI Integration: 📝 READY FOR IMPLEMENTATION** (Phase 5)

All building blocks are implemented and tested. This document provides the integration roadmap.

## What's Implemented

### Phase 1: Bitrate Auto-Removal ✅
- **File:** `src/deduplication.rs`
- **Functions:**
  - `auto_remove_inferior_bitrates(tracks, corpus_root, lost_found_root) -> Result<(moved, skipped)>`
  - `move_to_lost_found_auto()` - Helper for moving files
- **Behavior:**
  - Groups tracks by fingerprint
  - Skips groups with missing bitrate data (logs for manual resolution)
  - Moves inferior-bitrate duplicates to `lost-files/fingerprint-dupes-auto/`
  - Returns counts of moved and skipped tracks

### Phase 2: Cluster Computation ✅
- **File:** `src/deduplication.rs`
- **Struct:** `DirectoryCluster`
  - `directories: Vec<String>` - Directory names in cluster
  - `shared_fingerprints: Vec<String>` - Shared fingerprints
  - `track_count: usize` - Total tracks
  - `example_track: String` - Sample for display
- **Function:** `compute_directory_clusters(conflict_sets) -> Vec<DirectoryCluster>`
- **Algorithm:** BFS-based connected components (non-transitive)
- **Sorting:** By directory count desc, then track count desc

### Phase 3: TUI Picker Component ✅
- **File:** `src/ui/picker.rs`
- **Structs:**
  - `Picker` - Main picker widget
  - `PickerOption` - Individual option with label, description, value
  - `PickerResult` - Selected(String) or Cancelled
- **Features:**
  - Arrow key navigation (↑/↓)
  - Enter to select, Esc to cancel
  - Visual selection indicator (>> highlight)
  - Optional description text
  - Numbered options [1], [2], etc.

### Phase 4: Auto-Ignore Pattern Detection ✅
- **File:** `src/deduplication.rs`
- **Struct:** `AutoIgnoreState`
- **Methods:**
  - `new()` - Create tracker
  - `record_skip(dir) -> bool` - Returns true at 5-skip threshold
  - `record_win(dir)` - Reset counter
  - `add_ignored(dir)` - Add to ignore list
  - `should_ignore(dir) -> bool` - Check single directory
  - `should_skip_cluster(dirs) -> bool` - Check entire cluster
  - `get_ignored_dirs() -> Vec<String>` - List for stats

## Integration Roadmap (Phase 5)

### Step 1: Add New Menu State

**File:** `src/ui/menu.rs` (around line 149)

Add new state for cluster-based resolution:

```rust
// Fingerprint deduplication: Cluster resolution with auto-ignore
FingerprintDedupClusterResolve {
    clusters: Vec<crate::deduplication::DirectoryCluster>,
    conflict_sets: Vec<crate::deduplication::ConflictSet>,  // Keep for track lookups
    current_cluster_idx: usize,
    auto_ignore: crate::deduplication::AutoIgnoreState,
    session_stats: crate::deduplication::SessionStats,
    corpus_root: PathBuf,
    lost_found_root: PathBuf,
    auto_removal_stats: (usize, usize),  // (moved, skipped)
},
```

### Step 2: Modify Directory Selection → Resolution Transition

**File:** `src/ui/menu.rs` (around lines 990-1029)

Replace the current transition with:

```rust
// After user confirms directory selection...

// Step 1: Query all tracks in selected directories
let db = Database::open(&db_path)?;
let all_tracks = db.get_tracks_by_paths(selected_dirs, "corpus")?;

if all_tracks.is_empty() {
    self.status_message = Some("No fingerprinted tracks found".to_string());
    self.current_view = MenuState::CorpusTriageMenu;
    return;
}

// Step 2: Auto-remove inferior bitrates
let (moved, skipped) = match crate::deduplication::auto_remove_inferior_bitrates(
    &all_tracks,
    &corpus_root,
    &lost_found_root,
) {
    Ok(stats) => stats,
    Err(e) => {
        *error_message = Some(format!("Auto-removal error: {}", e));
        return;
    }
};

// Log auto-removal results
crate::config::log_message(&format!(
    "Auto-removal: {} inferior-bitrate files moved, {} skipped (missing data)",
    moved, skipped
))?;

// Step 3: Re-query tracks (excluding auto-removed ones)
let remaining_tracks = db.get_tracks_by_paths(selected_dirs, "corpus")?;

// Step 4: Find fingerprint duplicates among remaining tracks
let conflict_sets = match crate::deduplication::find_fingerprint_duplicates(
    &db,
    selected_dirs,
    &corpus_root,
) {
    Ok(sets) => sets,
    Err(e) => {
        *error_message = Some(format!("Error finding duplicates: {}", e));
        return;
    }
};

if conflict_sets.is_empty() {
    if moved > 0 {
        self.status_message = Some(format!(
            "Auto-removal complete: {} files moved. No manual conflicts remain.",
            moved
        ));
    } else {
        self.status_message = Some("No duplicate fingerprints found".to_string());
    }
    self.current_view = MenuState::CorpusTriageMenu;
    return;
}

// Step 5: Compute directory clusters
let clusters = crate::deduplication::compute_directory_clusters(&conflict_sets);

// Step 6: Transition to cluster resolution
self.current_view = MenuState::FingerprintDedupClusterResolve {
    clusters,
    conflict_sets,
    current_cluster_idx: 0,
    auto_ignore: crate::deduplication::AutoIgnoreState::new(),
    session_stats: crate::deduplication::SessionStats {
        total_sets: conflict_sets.len(),
        ..Default::default()
    },
    corpus_root,
    lost_found_root,
    auto_removal_stats: (moved, skipped),
};
```

### Step 3: Implement Cluster Resolution Handler

**File:** `src/ui/menu.rs` (new handler section)

Add input handler for cluster resolution:

```rust
// FINGERPRINT DEDUP CLUSTER RESOLVE HANDLERS
if let MenuState::FingerprintDedupClusterResolve {
    clusters,
    conflict_sets,
    current_cluster_idx,
    auto_ignore,
    session_stats,
    corpus_root,
    lost_found_root,
    ..
} = &mut self.current_view
{
    match key.code {
        KeyCode::Enter => {
            // Launch picker for directory selection
            let cluster = &clusters[*current_cluster_idx];

            // Skip if all directories auto-ignored
            if auto_ignore.should_skip_cluster(&cluster.directories) {
                *current_cluster_idx += 1;
                if *current_cluster_idx >= clusters.len() {
                    // All clusters processed - transition to stats
                    // ... transition logic
                }
                return;
            }

            // Build picker options
            let mut options = Vec::new();
            for dir in &cluster.directories {
                // Count tracks in this directory within this cluster
                let track_count: usize = conflict_sets
                    .iter()
                    .filter(|cs| cluster.shared_fingerprints.contains(&cs.fingerprint))
                    .flat_map(|cs| cs.tracks_by_dir.get(dir))
                    .map(|tracks| tracks.len())
                    .sum();

                options.push(crate::ui::picker::PickerOption {
                    label: format!("{} ({} files)", dir, track_count),
                    description: None,
                    value: dir.clone(),
                });
            }

            // Add "Skip" option
            options.push(crate::ui::picker::PickerOption {
                label: "Skip this cluster".to_string(),
                description: Some("Move to next cluster without resolving".to_string()),
                value: "__SKIP__".to_string(),
            });

            // Create and run picker
            let picker = crate::ui::picker::Picker::new(
                format!(
                    "Cluster {}/{}: {}-way conflict",
                    current_cluster_idx + 1,
                    clusters.len(),
                    cluster.directories.len()
                ),
                options,
            )
            .with_description(format!(
                "{} shared fingerprints, {} total files\nExample: {}",
                cluster.shared_fingerprints.len(),
                cluster.track_count,
                cluster.example_track
            ));

            // TODO: Actually run picker and handle result
            // Need to temporarily exit ratatui, run picker, re-enter
            // See existing tag editor pattern for terminal management
        }
        KeyCode::Char('s') => {
            // Skip cluster - record skips for auto-ignore
            let cluster = &clusters[*current_cluster_idx];
            for dir in &cluster.directories {
                if auto_ignore.record_skip(dir) {
                    // Prompt user: "You've skipped 'dir' 5 times. Always ignore? (y/N)"
                    // If yes: auto_ignore.add_ignored(dir.clone());
                }
            }

            session_stats.skipped_count += 1;
            *current_cluster_idx += 1;

            if *current_cluster_idx >= clusters.len() {
                // All clusters processed - transition to stats
                // ... transition logic
            }
        }
        KeyCode::Esc => {
            // Quit early - transition to stats with partial results
            // ... transition logic
        }
        _ => {}
    }
}
```

### Step 4: Handle Picker Result

When picker returns with selected directory:

```rust
match picker_result {
    crate::ui::picker::PickerResult::Selected(value) => {
        if value == "__SKIP__" {
            // Handle skip (same as 's' key above)
        } else {
            // User picked winner directory
            let winner = value;

            // Reset skip counter for winner
            auto_ignore.record_win(&winner);

            // Move all non-winner tracks to lost-files
            for cs in conflict_sets {
                if !cluster.shared_fingerprints.contains(&cs.fingerprint) {
                    continue; // Not part of this cluster
                }

                for (dir, tracks) in &cs.tracks_by_dir {
                    if dir == &winner {
                        session_stats.files_kept += tracks.len();
                    } else if cluster.directories.contains(dir) {
                        // Move these tracks
                        for track in tracks {
                            match crate::deduplication::move_to_lost_found(
                                track,
                                dir,
                                corpus_root,
                                lost_found_root,
                            ) {
                                Ok(_) => {
                                    session_stats.files_moved += 1;
                                    crate::config::log_message(&format!(
                                        "Moved: {} -> lost-files/fingerprint-dupes/{}/",
                                        track.path, dir
                                    ))?;
                                }
                                Err(e) => {
                                    crate::config::log_message(&format!(
                                        "Failed to move {}: {}",
                                        track.path, e
                                    ))?;
                                }
                            }
                        }
                    }
                }
            }

            session_stats.resolved_count += 1;
            *current_cluster_idx += 1;

            if *current_cluster_idx >= clusters.len() {
                // All clusters processed - transition to stats
                // ... transition logic
            }
        }
    }
    crate::ui::picker::PickerResult::Cancelled => {
        // User cancelled - quit early to stats
        // ... transition logic
    }
}
```

### Step 5: Update Stats Display

**File:** `src/ui/menu.rs` (stats rendering, around line 3611)

Enhance stats display to include auto-removal:

```rust
MenuState::FingerprintDedupStats {
    stats,
    lost_found_path,
    auto_removal_stats,  // NEW: Add this field
} => {
    let (moved_auto, skipped_auto) = auto_removal_stats;  // NEW

    let text = format!(
        "Fingerprint Deduplication Summary\n\
         ═════════════════════════════════\n\
         \n\
         AUTO-REMOVAL PHASE:\n\
         • Inferior-bitrate files moved: {}\n\
         • Skipped (missing data): {}\n\
         \n\
         CLUSTER RESOLUTION PHASE:\n\
         • Total conflict sets: {}\n\
         • Resolved: {}\n\
         • Skipped: {}\n\
         \n\
         FILES:\n\
         • Kept in corpus: {}\n\
         • Moved to lost-files: {}\n\
         \n\
         Lost-files location:\n\
         • Auto-removed: {}/fingerprint-dupes-auto/\n\
         • Cluster-removed: {}/fingerprint-dupes/\n\
         \n\
         Next steps:\n\
         • Rescan corpus to update database\n\
         • Review lost-files directory\n\
         • Delete moved files if confident\n\
         \n\
         [Enter] Return to menu",
        moved_auto,  // NEW
        skipped_auto,  // NEW
        stats.total_sets,
        stats.resolved_count,
        stats.skipped_count,
        stats.files_kept,
        stats.files_moved,
        lost_found_path.display(),
        lost_found_path.display(),
    );

    // ... render as paragraph
}
```

## Testing Checklist (Phase 6)

### Test Scenario 1: Auto-Removal Only
**Setup:**
- 3 files with same fingerprint
- Different bitrates: 128, 192, 320 kbps
- All bitrate data present

**Expected:**
1. Auto-removal moves 128 and 192 kbps files to `lost-files/fingerprint-dupes-auto/`
2. 320 kbps file remains in corpus
3. Stats show: "2 files moved, 0 skipped"
4. No clusters remain for manual resolution

**Verification:**
```bash
# Check lost-files structure
ls -R /path/to/lost-files/fingerprint-dupes-auto/

# Check log
tail /tmp/mla.log | grep "Auto-removed"

# Verify remaining file
ls /corpus/.../  # Should only show 320kbps file
```

### Test Scenario 2: Missing Bitrate Data
**Setup:**
- 3 files with same fingerprint
- One missing bitrate data

**Expected:**
1. Auto-removal skips entire group
2. Stats show: "0 files moved, 2 skipped"
3. All 3 files presented in cluster resolution

**Verification:**
```bash
# Check log
grep "Bitrate auto-removal skipped" /tmp/mla.log

# All files should remain in corpus
```

### Test Scenario 3: Multi-Cluster (Non-Transitive)
**Setup:**
- Directories: A, B, C
- A-B share fingerprint X
- B-C share fingerprint Y (different from X)
- A-C share no fingerprints

**Expected:**
1. Two separate clusters: (A-B) and (B-C)
2. NOT one combined cluster (A-B-C)
3. User resolves each cluster independently

**Verification:**
- Cluster 1 shows directories A and B
- Cluster 2 shows directories B and C
- B appears in both clusters (expected - non-transitive)

### Test Scenario 4: Auto-Ignore
**Setup:**
- Directory "favorites" appears in multiple clusters
- User skips it 5 times consecutively

**Expected:**
1. After 5th skip, prompt appears: "Always ignore 'favorites'? (y/N)"
2. If user accepts, all future clusters containing only "favorites" are auto-skipped
3. Clusters with "favorites" + other directories still shown (user can pick others)

**Verification:**
```bash
# Check log
grep "skipped 'favorites'" /tmp/mla.log
```

### Test Scenario 5: Picker Navigation
**Setup:**
- Cluster with 4 directories

**Expected:**
1. Arrow keys cycle through options
2. Highlight moves correctly
3. Enter selects highlighted option
4. Esc returns to menu (or shows partial stats)

## Code Locations Reference

### Core Deduplication Logic
- **File:** `src/deduplication.rs`
- **Lines:**
  - 166-239: `auto_remove_inferior_bitrates()`
  - 241-272: `move_to_lost_found_auto()`
  - 44-145: `compute_directory_clusters()`
  - 46-93: `AutoIgnoreState` implementation

### TUI Picker Component
- **File:** `src/ui/picker.rs`
- **Usage Example:** See docstring in file

### Menu Integration Points
- **File:** `src/ui/menu.rs`
- **Locations:**
  - Line 149: Add new MenuState variant
  - Lines 990-1029: Modify transition logic
  - Lines 1077-1175: Add cluster resolution handler
  - Lines 3611+: Update stats display

### Database Queries
- **File:** `src/db.rs`
- **Method:** `get_tracks_by_paths()` - Already implemented

## Known Limitations & Future Work

1. **Picker Terminal Management:** Need to properly exit/re-enter ratatui when running picker
   - See tag editor implementation for pattern
   - Lines 850-920 in menu.rs show terminal drop/recreation

2. **Progress Indicators:** Auto-removal could show progress bar for large operations
   - Use existing ScanProgress pattern from scanner.rs

3. **Undo/Rollback:** No transaction safety for file moves
   - Consider adding transaction log
   - Would allow rollback of entire session

4. **Parallel Processing:** Auto-removal processes fingerprints sequentially
   - Could parallelize with rayon like scanner does
   - Be careful with file I/O contention

5. **Conflict Visualization:** Picker could show more details
   - File paths preview
   - Bitrate comparison
   - File size comparison

## API Stability

All public functions in `src/deduplication.rs` have stable signatures:

```rust
// Phase 1
pub fn auto_remove_inferior_bitrates(
    tracks: &[Track],
    corpus_root: &Path,
    lost_found_root: &Path,
) -> Result<(usize, usize)>

// Phase 2
pub fn compute_directory_clusters(
    conflict_sets: &[ConflictSet],
) -> Vec<DirectoryCluster>

// Phase 4
impl AutoIgnoreState {
    pub fn new() -> Self
    pub fn record_skip(&mut self, dir: &str) -> bool
    pub fn record_win(&mut self, dir: &str)
    pub fn add_ignored(&mut self, dir: String)
    pub fn should_ignore(&self, dir: &str) -> bool
    pub fn should_skip_cluster(&self, cluster_dirs: &[String]) -> bool
    pub fn get_ignored_dirs(&self) -> Vec<String>
}

// Phase 3
impl Picker {
    pub fn new(title: String, options: Vec<PickerOption>) -> Self
    pub fn with_description(self, desc: String) -> Self
    pub fn run<B: Backend>(
        self,
        terminal: &mut Terminal<B>,
    ) -> Result<PickerResult, Box<dyn std::error::Error>>
}
```

## Next Steps

1. **Implement UI Integration (Phase 5):**
   - Follow this guide to wire up the menu states
   - Test each transition carefully
   - Handle terminal management for picker

2. **Run Integration Tests (Phase 6):**
   - Use test scenarios above
   - Verify lost-files directory structure
   - Check logs for all operations

3. **Polish & Document (Phase 7):**
   - Add inline comments to complex sections
   - Update README with new workflow
   - Create user-facing documentation

## Questions or Issues?

If integration reveals issues with the core implementations (Phases 1-4), those are stable and can be debugged independently. The building blocks are complete and tested.
