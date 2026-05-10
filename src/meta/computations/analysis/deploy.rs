//! Deploy-related executors.
//!
//! Deploy conflict detection, deploy health signals, and corpus deploy status.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::corpus::deploy::{
    compute_deployment_path_with_tags, deploy_album_directory, extract_release_directory,
};
use crate::db::write_thread;
use crate::db::ReadOnlyDb;
use crate::logging::{log_error, log_general};
use crate::meta::computations::helpers::is_image_file;
use crate::meta::computations::helpers::{
    reconcile_aggregate_signals, reconcile_corpus_signals, ComputedAggregateSignal,
    ComputedCorpusSignal,
};
use crate::meta::computations::traits::ComputationContext;
use crate::meta::computations::types::ComputationWitness;
use crate::meta::signals::data::{
    DeployConflictSignal, DeployLifecyclePhase, DeployReadySignal, DeployedHealthySignal,
    LibraryLeftoverSignal, LibraryStaleSignal, ReleaseOverlapData, ReleaseOverlapEntry,
    ReleaseOverlapSignal, SidecarDeployConflictSignal, SidecarDeployReadyData,
    SidecarDeployReadySignal};
use crate::meta::signals::registry::TypedSignalWrite;

use super::{Computation, Result};

/// A corpus file with its precomputed deploy path and library association.
///
/// Used by `DeriveCorpusDeployStatus` to batch-process deploy status, and by
/// `derive_sidecar_deploy_signals` to discover directories needing sidecars.
struct PrecomputedFile {
    inode: i64,
    corpus_path: String,
    deploy_path: String,
    library_name: String,
}

// ============================================================================
// Deploy Conflict Detection
// ============================================================================

/// Execute DetectDeployConflicts - bulk detection of deploy path collisions.
pub fn execute_detect_deploy_conflicts(
    ctx: &ComputationContext<'_>,
) -> Result {
    let read_only_db = ctx.read_db;
    let witness = ctx.witness;
    let computation = Computation::DetectDeployConflicts;

    let sender = require_sender!(computation);

    let healthy_signals = match read_only_db.get_healthy_file_signals() {
        Ok(v) => v,
        Err(e) => {
            log_error(format!(
                "[COMPUTE] DetectDeployConflicts: get_healthy_file_signals failed: {}",
                e
            ));
            return Result::failure(
                computation,
                format!("get_healthy_file_signals: {}", e),
            );
        }
    };

    let mut deploy_path_to_tracks: HashMap<String, Vec<i64>> = HashMap::new();

    for signal in &healthy_signals {
        let tags = match read_only_db.get_tags::<crate::zones::CorpusZone>(signal.inode) {
            Ok(v) => v,
            Err(e) => {
                log_error(format!(
                    "[COMPUTE] DetectDeployConflicts: get_corpus_tags failed for inode {}: {}",
                    signal.inode, e
                ));
                Vec::new()
            }
        };
        let tag_map = crate::meta::computations::helpers::tags_to_map(tags);

        let deploy_path = compute_deployment_path_with_tags(&signal.path, &tag_map)
            .to_string_lossy()
            .to_string();

        deploy_path_to_tracks
            .entry(deploy_path)
            .or_default()
            .push(signal.inode);
    }

    let mut computed: Vec<ComputedAggregateSignal> = Vec::new();
    for (deploy_path, inodes) in &deploy_path_to_tracks {
        if inodes.len() > 1 {
            let signal = TypedSignalWrite::DeployConflict(DeployConflictSignal {
                key: deploy_path.clone(),
                inodes: inodes.clone(),
            });
            computed.push(ComputedAggregateSignal::new(deploy_path.clone(), signal));
        }
    }

    let conflict_count = computed.len();
    let stats = reconcile_aggregate_signals::<DeployConflictSignal>(
        read_only_db,
        &sender,
        computed,
        witness,
    );

    log_general(format!(
        "[COMPUTE] DetectDeployConflicts: {} conflicts among {} healthy files (reconcile: {})",
        conflict_count,
        healthy_signals.len(),
        stats,
    ));

    Result::success(computation, Vec::new())
}

// ============================================================================
// Release Overlap Detection
// ============================================================================

/// Execute DetectReleaseOverlaps - detect cross-source album-directory release overlaps.
///
/// Groups corpus files by their computed album directory (parent of deploy path),
/// then partitions by (source_dir, release_dir). When multiple configured sources
/// target the same album directory, emits a ReleaseOverlapSignal.
/// Intra-source overlaps (same source, different release dirs) are skipped.
pub fn execute_detect_release_overlaps(
    ctx: &ComputationContext<'_>,
) -> Result {
    let read_only_db = ctx.read_db;
    let witness = ctx.witness;
    let computation = Computation::DetectReleaseOverlaps;

    let sender = require_sender!(computation);

    let config = require_config!(ctx, computation);

    let healthy_signals = match read_only_db.get_healthy_file_signals() {
        Ok(v) => v,
        Err(e) => {
            log_error(format!(
                "[COMPUTE] DetectReleaseOverlaps: get_healthy_file_signals failed: {}",
                e
            ));
            return Result::failure(
                computation,
                format!("get_healthy_file_signals: {}", e),
            );
        }
    };

    // For each corpus file: compute deploy path → album directory, identify source+release.
    // Key: (source_dir, release_dir), grouped by album directory.
    struct FileInfo {
        inode: i64,
        corpus_path: String,
        source_dir: String,
        release_dir: String,
        can_stash: bool,
    }

    // album_dir → Vec<FileInfo>
    let mut album_dir_files: HashMap<String, Vec<FileInfo>> = HashMap::new();

    for signal in &healthy_signals {
        let resolved = match config.resolve_source_config(Path::new(&signal.path)) {
            Some(r) => r,
            None => continue,
        };

        let tags = match read_only_db.get_tags::<crate::zones::CorpusZone>(signal.inode) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if tags.is_empty() {
            continue;
        }

        let tag_map = crate::meta::computations::helpers::tags_to_map(tags);

        let deploy_path = compute_deployment_path_with_tags(&signal.path, &tag_map)
            .to_string_lossy()
            .to_string();

        let album_dir = deploy_album_directory(&deploy_path);
        if album_dir.is_empty() || album_dir.starts_with("[no album artist]") {
            continue;
        }

        let source_dir = resolved.source_path.to_string_lossy().to_string();
        let release_dir = extract_release_directory(&signal.path, &resolved.source_path);

        album_dir_files
            .entry(album_dir)
            .or_default()
            .push(FileInfo {
                inode: signal.inode,
                corpus_path: signal.path.clone(),
                source_dir,
                release_dir,
                can_stash: resolved.can_stash_dupes,
            });
    }

    // Build signals: for each album directory with 2+ distinct (source, release) pairs.
    let mut computed: Vec<ComputedAggregateSignal> = Vec::new();

    for (album_dir, files) in &album_dir_files {
        // Group by (source_dir, release_dir)
        let mut release_groups: HashMap<(&str, &str), Vec<&FileInfo>> = HashMap::new();
        for f in files {
            release_groups
                .entry((&f.source_dir, &f.release_dir))
                .or_default()
                .push(f);
        }

        if release_groups.len() < 2 {
            continue; // Single release — normal album, no overlap
        }

        // Only cross-source overlaps are actionable.
        let distinct_sources: HashSet<&str> = release_groups.keys().map(|(s, _)| *s).collect();
        if distinct_sources.len() < 2 {
            continue;
        }

        let mut releases: Vec<ReleaseOverlapEntry> = Vec::new();
        let mut total_files = 0usize;

        for ((source_dir, release_dir), group_files) in &release_groups {
            let inodes: Vec<i64> = group_files.iter().map(|f| f.inode).collect();
            let corpus_paths: Vec<String> =
                group_files.iter().map(|f| f.corpus_path.clone()).collect();
            let can_stash = group_files.first().map(|f| f.can_stash).unwrap_or(false);
            total_files += inodes.len();

            releases.push(ReleaseOverlapEntry {
                source_dir: source_dir.to_string(),
                release_dir: release_dir.to_string(),
                can_stash,
                inodes,
                corpus_paths,
            });
        }

        // Sort releases for deterministic ordering
        releases
            .sort_by(|a, b| (&a.source_dir, &a.release_dir).cmp(&(&b.source_dir, &b.release_dir)));

        let signal = TypedSignalWrite::ReleaseOverlap(ReleaseOverlapSignal {
            key: album_dir.clone(),
            data: ReleaseOverlapData {
                releases,
                file_count: total_files,
            },
        });
        computed.push(ComputedAggregateSignal::new(album_dir.clone(), signal));
    }

    let overlap_count = computed.len();
    let stats = reconcile_aggregate_signals::<ReleaseOverlapSignal>(
        read_only_db,
        &sender,
        computed,
        witness,
    );

    log_general(format!(
        "[COMPUTE] DetectReleaseOverlaps: {} overlapping album dirs among {} dirs ({} healthy files) (reconcile: {})",
        overlap_count,
        album_dir_files.len(),
        healthy_signals.len(),
        stats,
    ));

    // DeriveCorpusDeployStatus reads ReleaseOverlap signals written above.
    // Defer it behind a barrier so the Witch drains all in-flight work + DB
    // writes before spawning it.
    Result::pipeline(
        computation,
        vec![],
        vec![(
            super::super::PipelineStage::DependentAnalysis,
            vec![super::super::Computation::Analysis(
                Computation::DeriveCorpusDeployStatus,
            )],
        )],
    )
}

// ============================================================================
// Deploy Health Signals
// ============================================================================

/// Execute DeriveDeployHealthSignals - derive library health from scan data.
///
/// Reads library file data from files table (zone='library') and compares against
/// corpus index to identify leftovers and stale deployments.
pub fn execute_derive_deploy_health_signals(
    ctx: &ComputationContext<'_>,
    library_name: &str,
    library_root: &PathBuf,
    corpus_path_prefixes: &[PathBuf],
) -> Result {
    let read_only_db = ctx.read_db;
    let witness = ctx.witness;
    let computation = Computation::DeriveDeployHealthSignals {
        library_name: library_name.to_string(),
        library_root: library_root.to_path_buf(),
        corpus_path_prefixes: corpus_path_prefixes.to_vec(),
    };

    let sender = require_sender!(computation);

    log_general(format!(
        "[COMPUTE] DeriveDeployHealthSignals '{}': starting (root={:?}, prefixes={:?})",
        library_name, library_root, corpus_path_prefixes,
    ));

    // Bulk clear all existing leftover/stale signals for this library before recomputing.
    // This prevents stale signals from persisting when files are removed between runs.
    let leftover_prefix = LibraryLeftoverSignal::key_prefix_for_library(library_name);
    let stale_prefix = LibraryStaleSignal::key_prefix_for_library(library_name);
    sender.clear_aggregate_by_key_prefix::<LibraryLeftoverSignal>(&leftover_prefix, witness);
    sender.clear_aggregate_by_key_prefix::<LibraryStaleSignal>(&stale_prefix, witness);

    // Query library file data from Derivation phase
    let library_scan_entries = match read_only_db.get_library_files(library_name) {
        Ok(entries) => entries,
        Err(e) => {
            log_general(format!(
                "[COMPUTE] DeriveDeployHealthSignals '{}': failed to get scan data: {}",
                library_name, e
            ));
            return Result::success(computation, Vec::new());
        }
    };

    log_general(format!(
        "[COMPUTE] DeriveDeployHealthSignals '{}': got {} library scan entries from DB",
        library_name,
        library_scan_entries.len(),
    ));

    // Log first few entries for debugging
    for (i, entry) in library_scan_entries.iter().take(3).enumerate() {
        log_general(format!(
            "[COMPUTE] DeriveDeployHealthSignals '{}': sample lib[{}] path={:?} inode={}",
            library_name, i, entry.file_path, entry.inode,
        ));
    }

    // Convert to (path, inode) tuples for processing
    let library_files: Vec<(std::path::PathBuf, i64)> = library_scan_entries
        .into_iter()
        .map(|entry| (entry.file_path, entry.inode))
        .collect();

    // Get all corpus audio file inodes
    let corpus_inodes = match read_only_db.get_all_inodes::<crate::zones::CorpusZone>() {
        Ok(v) => v,
        Err(e) => {
            log_error(format!(
                "[COMPUTE] DeriveDeployHealthSignals '{}': get_all_corpus_inodes failed: {}",
                library_name, e
            ));
            return Result::failure(
                computation,
                format!("get_all_corpus_inodes: {}", e),
            );
        }
    };

    log_general(format!(
        "[COMPUTE] DeriveDeployHealthSignals '{}': {} corpus inodes loaded",
        library_name,
        corpus_inodes.len(),
    ));

    // Preload the set of corpus inodes that have audio_info rows. This replaces
    // a per-file `get_audio_file_by_path(corpus_path)` lookup inside the hot loop
    // (87k 3-table joins for the music library) with a HashSet membership test.
    // Library files are hardlinked to their corpus origins, so library_inode is
    // the corpus inode and `audio_info` membership answers "is this audio?".
    let corpus_audio_inodes: HashSet<i64> = match read_only_db
        .get_audio_inodes_for_zone(crate::db::types::Zone::Corpus)
    {
        Ok(set) => set,
        Err(e) => {
            log_error(format!(
                "[COMPUTE] DeriveDeployHealthSignals '{}': get_audio_inodes_for_zone failed: {}",
                library_name, e,
            ));
            return Result::failure(
                computation,
                format!("get_audio_inodes_for_zone: {}", e),
            );
        }
    };
    log_general(format!(
        "[COMPUTE] DeriveDeployHealthSignals '{}': {} corpus audio inodes preloaded",
        library_name,
        corpus_audio_inodes.len(),
    ));

    // Build path→inode index for conflict detection.
    // When a stale file's expected path is already occupied by a different inode,
    // it's a stale-conflict (tag edit moved the expected path onto an occupied slot).
    // These are masked like deploy conflicts — emitting a stale signal would just
    // produce a LibraryMove that fails every cycle.
    let library_path_to_inode: HashMap<PathBuf, i64> = library_files
        .iter()
        .map(|(path, inode)| (path.clone(), *inode))
        .collect();

    // Bulk-load corpus tags for every library inode in one chunked IN query,
    // replacing per-file `get_tags(inode)` round trips inside `classify_audio_stale`
    // and `compute_expected_library_path`. Library files share inodes with their
    // corpus origins via hardlinks, so the library inode set is the lookup key.
    let unique_inodes: Vec<i64> = {
        let mut set: HashSet<i64> = HashSet::new();
        for (_, inode) in &library_files {
            set.insert(*inode);
        }
        set.into_iter().collect()
    };
    let corpus_tag_maps: HashMap<i64, HashMap<String, String>> = match read_only_db
        .get_tags_batch_for_zone(&unique_inodes, crate::db::types::Zone::Corpus)
    {
        Ok(rows) => rows
            .into_iter()
            .map(|(inode, tags)| {
                let tag_map: HashMap<String, String> = tags
                    .into_iter()
                    .map(|(name, value)| (name.to_uppercase(), value))
                    .collect();
                (inode, tag_map)
            })
            .collect(),
        Err(e) => {
            log_error(format!(
                "[COMPUTE] DeriveDeployHealthSignals '{}': get_tags_batch_for_zone failed: {} (will fall back to per-inode queries)",
                library_name, e,
            ));
            HashMap::new()
        }
    };
    log_general(format!(
        "[COMPUTE] DeriveDeployHealthSignals '{}': preloaded tags for {} unique corpus inodes",
        library_name,
        corpus_tag_maps.len(),
    ));

    // Lazy cache: corpus directory → Option<album_dir> for sidecar stale detection.
    // Populated on first image encounter per directory, avoids redundant lookups.
    let mut dir_to_album_dir: HashMap<String, Option<String>> = HashMap::new();

    let mut healthy_count: usize = 0;
    let mut stale_count: usize = 0;
    let mut sidecar_stale_count: usize = 0;
    let mut leftover_count: usize = 0;
    let stale_conflict_count: usize = 0;
    let mut debug_logged = 0usize;

    for (library_path, library_inode) in &library_files {
        // Log first few inode lookups to trace match/miss behavior
        if debug_logged < 5 {
            let corpus_match = corpus_inodes.get(library_inode);
            log_general(format!(
                "[COMPUTE] DeriveDeployHealthSignals '{}': lib inode {} path={:?} -> corpus={:?}",
                library_name, library_inode, library_path, corpus_match,
            ));
            debug_logged += 1;
        }

        let library_path_display = library_path.display().to_string();

        // Classify lifecycle phase — Leftover if no corpus backing, else delegate
        let (phase, corpus_path) = match corpus_inodes.get(library_inode) {
            Some(corpus_path) => {
                let phase = classify_library_file(
                    read_only_db,
                    library_name,
                    library_path,
                    *library_inode,
                    corpus_path,
                    &corpus_audio_inodes,
                    &library_path_to_inode,
                    &mut dir_to_album_dir,
                    &corpus_tag_maps,
                );
                (phase, Some(corpus_path))
            }
            None => (DeployLifecyclePhase::Leftover, None),
        };

        match phase {
            DeployLifecyclePhase::Healthy => {
                healthy_count += 1;
            }
            DeployLifecyclePhase::Stale => {
                let corpus_path = corpus_path.expect("Stale implies corpus backing");
                let expected_with_prefix = compute_expected_library_path(
                    read_only_db,
                    library_name,
                    *library_inode,
                    corpus_path,
                    &corpus_audio_inodes,
                    &mut dir_to_album_dir,
                    &corpus_tag_maps,
                );
                if let Some(expected_path) = expected_with_prefix {
                    let is_image = is_image_file(Path::new(corpus_path));
                    let stale_key =
                        LibraryStaleSignal::make_key(library_name, &library_path_display);
                    sender.write_typed_signal(
                        TypedSignalWrite::LibraryStale(LibraryStaleSignal {
                            key: stale_key,
                            library_path: library_path_display,
                            expected_path,
                            corpus_path: corpus_path.clone(),
                            inode: *library_inode,
                        }),
                        witness,
                    );
                    if is_image {
                        sidecar_stale_count += 1;
                    } else {
                        stale_count += 1;
                    }
                } else {
                    healthy_count += 1;
                }
            }
            DeployLifecyclePhase::Leftover => {
                leftover_count += 1;
                let leftover_key =
                    LibraryLeftoverSignal::make_key(library_name, &library_path_display);
                sender.write_typed_signal(
                    TypedSignalWrite::LibraryLeftover(LibraryLeftoverSignal { key: leftover_key }),
                    witness,
                );
            }
            DeployLifecyclePhase::Ready => {
                // Library files are already deployed — Ready is a corpus-side phase.
                // If we ever reach this, the classification logic has a bug.
                unreachable!("Library file cannot be in Ready phase");
            }
        }
    }

    log_general(format!(
        "[COMPUTE] DeriveDeployHealthSignals '{}': {} files, {} healthy, {} stale ({}a + {}i), {} leftover, {} stale-conflict",
        library_name,
        library_files.len(),
        healthy_count,
        stale_count + sidecar_stale_count,
        stale_count,
        sidecar_stale_count,
        leftover_count,
        stale_conflict_count,
    ));

    Result::success(computation, Vec::new())
}

// ============================================================================
// Library File Classification Helpers
// ============================================================================

/// Classify a corpus file's deploy lifecycle phase.
///
/// Checks whether the file's inode exists in any library and whether the
/// library path matches the expected deploy path.
/// Returns Ready (not deployed), Healthy (correctly deployed), or Stale
/// (deployed at wrong path). Never returns Leftover (that's library-side).
fn classify_corpus_file(
    file: &PrecomputedFile,
    library_inode_to_paths: &HashMap<i64, Vec<PathBuf>>,
) -> DeployLifecyclePhase {
    let Some(library_paths) = library_inode_to_paths.get(&file.inode) else {
        return DeployLifecyclePhase::Ready;
    };

    // File is in library — check if any library path matches expected
    let has_match = library_paths.iter().any(|lp| {
        let components: Vec<_> = lp.components().collect();
        if components.len() > 1 {
            let suffix: PathBuf = components[1..].iter().collect();
            suffix == Path::new(&file.deploy_path)
        } else {
            false
        }
    });

    if has_match {
        DeployLifecyclePhase::Healthy
    } else {
        DeployLifecyclePhase::Stale
    }
}

/// Classify a library file's deploy lifecycle phase.
///
/// For audio files: looks up audio_info + tags to compute expected deploy path.
/// For image files: finds audio siblings to derive the expected album directory.
/// Returns the lifecycle phase; signal emission is handled by the caller.
fn classify_library_file(
    read_only_db: &ReadOnlyDb<'_>,
    library_name: &str,
    library_path: &Path,
    library_inode: i64,
    corpus_path: &str,
    corpus_audio_inodes: &HashSet<i64>,
    library_path_to_inode: &HashMap<PathBuf, i64>,
    dir_to_album_dir: &mut HashMap<String, Option<String>>,
    corpus_tag_maps: &HashMap<i64, HashMap<String, String>>,
) -> DeployLifecyclePhase {
    // Library files are hardlinked to corpus, so library_inode == corpus inode.
    // A membership check on the preloaded audio set replaces a per-file 3-table join.
    if corpus_audio_inodes.contains(&library_inode) {
        return classify_audio_stale(
            read_only_db,
            library_name,
            library_path,
            library_inode,
            corpus_path,
            library_path_to_inode,
            corpus_tag_maps,
        );
    }

    // Not an audio file — check if it's a sidecar image
    if !is_image_file(Path::new(corpus_path)) {
        return DeployLifecyclePhase::Healthy; // Unknown file type, treat as healthy
    }

    // Image file: derive expected path from audio siblings' tags
    check_sidecar_stale(
        read_only_db,
        library_name,
        library_path,
        library_inode,
        corpus_path,
        library_path_to_inode,
        dir_to_album_dir,
        corpus_tag_maps,
    )
}

/// Look up a corpus inode's tag map, preferring the precomputed batch and
/// falling back to a single per-inode query if absent (defense in depth).
fn corpus_tags_for(
    read_only_db: &ReadOnlyDb<'_>,
    inode: i64,
    corpus_tag_maps: &HashMap<i64, HashMap<String, String>>,
) -> HashMap<String, String> {
    if let Some(map) = corpus_tag_maps.get(&inode) {
        return map.clone();
    }
    let tags = read_only_db
        .get_tags::<crate::zones::CorpusZone>(inode)
        .unwrap_or_default();
    crate::meta::computations::helpers::tags_to_map(tags)
}

/// Classify an audio library file as stale or healthy by comparing deploy paths.
fn classify_audio_stale(
    read_only_db: &ReadOnlyDb<'_>,
    library_name: &str,
    library_path: &Path,
    library_inode: i64,
    corpus_path: &str,
    library_path_to_inode: &HashMap<PathBuf, i64>,
    corpus_tag_maps: &HashMap<i64, HashMap<String, String>>,
) -> DeployLifecyclePhase {
    // Library inode == corpus inode for hardlinked deployments.
    let tag_map = corpus_tags_for(read_only_db, library_inode, corpus_tag_maps);

    let expected_relative = compute_deployment_path_with_tags(corpus_path, &tag_map);
    let library_path_suffix = library_path
        .strip_prefix(library_name)
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|_| library_path.to_path_buf());

    if library_path_suffix == expected_relative {
        return DeployLifecyclePhase::Healthy;
    }

    // Path mismatch — check for stale-conflict or duplicate hardlink
    let expected_with_prefix = Path::new(library_name).join(&expected_relative);
    if let Some(&occupant_inode) = library_path_to_inode.get(&expected_with_prefix) {
        if occupant_inode != library_inode {
            return DeployLifecyclePhase::Healthy; // Stale-conflict: mask as healthy
        } else {
            // Same inode already exists at the correct path — this old path is cruft
            return DeployLifecyclePhase::Leftover;
        }
    }

    DeployLifecyclePhase::Stale
}

/// Check if a sidecar image in the library is stale.
///
/// Derives the expected library path by finding an audio sibling in the same
/// corpus directory, computing its deploy album directory from tags, then
/// comparing against the actual library path.
fn check_sidecar_stale(
    read_only_db: &ReadOnlyDb<'_>,
    library_name: &str,
    library_path: &Path,
    library_inode: i64,
    corpus_path: &str,
    library_path_to_inode: &HashMap<PathBuf, i64>,
    dir_to_album_dir: &mut HashMap<String, Option<String>>,
    corpus_tag_maps: &HashMap<i64, HashMap<String, String>>,
) -> DeployLifecyclePhase {
    let corpus_dir = Path::new(corpus_path)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    // Lazily populate album_dir for this corpus directory
    let album_dir = if let Some(v) = dir_to_album_dir.get(&corpus_dir) {
        v.clone()
    } else {
        let v = lookup_album_dir_from_sibling(read_only_db, &corpus_dir, corpus_tag_maps);
        dir_to_album_dir.entry(corpus_dir).or_insert(v).clone()
    };

    let album_dir = match album_dir {
        Some(dir) => dir,
        None => return DeployLifecyclePhase::Healthy, // No audio siblings — can't determine expected path
    };

    // Expected library path: album_dir/filename
    let filename = Path::new(corpus_path)
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_default();

    if filename.is_empty() {
        return DeployLifecyclePhase::Healthy;
    }

    let expected_relative = format!("{}/{}", album_dir, filename);
    let library_path_suffix = library_path
        .strip_prefix(library_name)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| library_path.to_string_lossy().to_string());

    // Normalize: strip leading slash from suffix if present
    let library_path_suffix = library_path_suffix.trim_start_matches('/');

    if library_path_suffix == expected_relative {
        return DeployLifecyclePhase::Healthy;
    }

    // Path mismatch — check for stale-conflict or duplicate hardlink
    let expected_with_prefix = Path::new(library_name).join(&expected_relative);
    if let Some(&occupant_inode) = library_path_to_inode.get(&expected_with_prefix) {
        if occupant_inode != library_inode {
            return DeployLifecyclePhase::Healthy; // Stale-conflict: mask as healthy
        } else {
            // Same inode already exists at the correct path — this old path is cruft
            return DeployLifecyclePhase::Leftover;
        }
    }

    DeployLifecyclePhase::Stale
}

/// Look up the deploy album directory for a corpus directory by finding an audio
/// sibling and computing its deploy path from tags.
///
/// Reads the sibling's tag map from `corpus_tag_maps` when present (sidecar
/// directories almost always have an audio sibling that is itself a library
/// file, so its tags are in the precomputed batch). Falls back to a per-inode
/// query for the rare miss.
fn lookup_album_dir_from_sibling(
    read_only_db: &ReadOnlyDb<'_>,
    corpus_dir: &str,
    corpus_tag_maps: &HashMap<i64, HashMap<String, String>>,
) -> Option<String> {
    let (sibling_inode, sibling_path) = read_only_db
        .get_any_audio_sibling_in_directory(corpus_dir)
        .ok()??;

    let tag_map = corpus_tags_for(read_only_db, sibling_inode, corpus_tag_maps);
    if tag_map.is_empty() {
        return None;
    }

    let deploy_path = compute_deployment_path_with_tags(&sibling_path, &tag_map);
    let album_dir = deploy_album_directory(&deploy_path.to_string_lossy());

    if album_dir.is_empty() {
        None
    } else {
        Some(album_dir)
    }
}

/// Compute the expected library path for a file (audio or image).
///
/// Used after classification determines a file is stale, to get the
/// expected path for the LibraryStaleSignal.
fn compute_expected_library_path(
    read_only_db: &ReadOnlyDb<'_>,
    library_name: &str,
    library_inode: i64,
    corpus_path: &str,
    corpus_audio_inodes: &HashSet<i64>,
    dir_to_album_dir: &mut HashMap<String, Option<String>>,
    corpus_tag_maps: &HashMap<i64, HashMap<String, String>>,
) -> Option<String> {
    // Audio file: compute from tags directly. Audio membership is the inode set
    // (library_inode == corpus inode for hardlinks), no per-file SQL needed.
    if corpus_audio_inodes.contains(&library_inode) {
        let tag_map = corpus_tags_for(read_only_db, library_inode, corpus_tag_maps);
        let expected_relative = compute_deployment_path_with_tags(corpus_path, &tag_map);
        let expected_with_prefix = Path::new(library_name).join(&expected_relative);
        return Some(expected_with_prefix.to_string_lossy().to_string());
    }

    // Image file: derive from sibling album_dir
    let corpus_dir = Path::new(corpus_path)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    let album_dir = if let Some(v) = dir_to_album_dir.get(&corpus_dir) {
        v.clone()?
    } else {
        let v = lookup_album_dir_from_sibling(read_only_db, &corpus_dir, corpus_tag_maps);
        dir_to_album_dir.entry(corpus_dir).or_insert(v).clone()?
    };

    let filename = Path::new(corpus_path)
        .file_name()
        .map(|f| f.to_string_lossy().to_string())?;

    let expected_with_prefix = format!("{}/{}/{}", library_name, album_dir, filename);
    Some(expected_with_prefix)
}

// ============================================================================
// Corpus Deploy Status
// ============================================================================

/// Execute DeriveCorpusDeployStatus - derive corpus-side deployment signals.
///
/// For each HealthyFile signal, checks if the file's inode exists in any library
/// (via files table) and emits:
/// - DeployReady: healthy file not in any library
/// - DeployedHealthy: healthy file correctly deployed (in library, not stale)
pub fn execute_derive_corpus_deploy_status(
    ctx: &ComputationContext<'_>,
) -> Result {
    let read_only_db = ctx.read_db;
    let witness = ctx.witness;
    let computation = Computation::DeriveCorpusDeployStatus;

    let sender = require_sender!(computation);

    // Incremental short-circuit: if no inodes have been marked dirty for
    // corpus_deploy_status AND prior signals exist (i.e. we have run before
    // and the result set is non-empty), skip the full corpus scan.
    //
    // Dirty marking fires under TAGS|FILES|DEPLOY scope mutations (see
    // witch/execution.rs Phase 1c), which covers every state change that can
    // flip a corpus inode's deploy classification. An empty dirty set therefore
    // means no inode has changed state since the last successful run.
    {
        let dirty = read_only_db
            .get_dirty_inodes(crate::meta::computations::CORPUS_DEPLOY_STATUS_COMPUTATION)
            .unwrap_or_default();
        if dirty.is_empty() {
            let dr_count = read_only_db
                .corpus_signal_all_inodes::<DeployReadySignal>()
                .map(|v| v.len())
                .unwrap_or(0);
            let dh_count = read_only_db
                .corpus_signal_all_inodes::<DeployedHealthySignal>()
                .map(|v| v.len())
                .unwrap_or(0);
            if dr_count + dh_count > 0 {
                log_general(format!(
                    "[COMPUTE] DeriveCorpusDeployStatus: skipped (no dirty corpus inodes; \
                     {} DeployReady + {} DeployedHealthy signals retained)",
                    dr_count, dh_count,
                ));
                return Result::success(computation, Vec::new());
            }
        }
    }

    // Get all HealthyFile signals
    let healthy_signals = match read_only_db.get_healthy_file_signals() {
        Ok(v) => v,
        Err(e) => {
            log_error(format!(
                "[COMPUTE] DeriveCorpusDeployStatus: get_healthy_file_signals failed: {}",
                e
            ));
            return Result::failure(
                computation,
                format!("get_healthy_file_signals: {}", e),
            );
        }
    };

    // Build inode → library paths map from files table (zone='library')
    // This replaces the old stale_signals/stale_inodes approach that had a race
    // condition with DeriveDeployHealthSignals. We compute stale status inline.
    let mut library_inode_to_paths: HashMap<i64, Vec<PathBuf>> = HashMap::new();
    match read_only_db.get_all_library_files() {
        Ok(all_library_files) => {
            log_general(format!(
                "[COMPUTE] DeriveCorpusDeployStatus: loaded {} library files from DB",
                all_library_files.len(),
            ));
            for entry in all_library_files {
                library_inode_to_paths
                    .entry(entry.inode)
                    .or_default()
                    .push(entry.file_path);
            }
        }
        Err(e) => {
            log_error(format!(
                "[COMPUTE] DeriveCorpusDeployStatus: get_all_library_files failed: {}",
                e,
            ));
        }
    }

    // Get deploy-configured corpus paths to filter healthy files
    let config = require_config!(ctx, computation);

    // Phase 1: Compute deploy paths for ALL indexed corpus files to build
    // the collision map.  A file cannot be DeployReady if ANY other corpus
    // inode computes the same deploy path — regardless of health status.
    // Only healthy, source-configured files go into `precomputed` for signal
    // emission, but deploy_path_counts covers the entire corpus.

    let all_corpus_inodes = match read_only_db.get_all_inodes::<crate::zones::CorpusZone>() {
        Ok(v) => v,
        Err(e) => {
            log_error(format!(
                "[COMPUTE] DeriveCorpusDeployStatus: get_all_corpus_inodes failed: {}",
                e
            ));
            return Result::failure(
                computation,
                format!("get_all_corpus_inodes: {}", e),
            );
        }
    };

    let healthy_inodes: HashSet<i64> = healthy_signals.iter().map(|s| s.inode).collect();

    // Bulk-load tags for every corpus inode in chunked IN queries. Replaces
    // ~100k individual `get_tags(inode)` round trips inside the loop below.
    let inode_keys: Vec<i64> = all_corpus_inodes.keys().copied().collect();
    let corpus_tag_maps: HashMap<i64, HashMap<String, String>> = match read_only_db
        .get_tags_batch_for_zone(&inode_keys, crate::db::types::Zone::Corpus)
    {
        Ok(rows) => rows
            .into_iter()
            .map(|(inode, tags)| {
                let map: HashMap<String, String> = tags
                    .into_iter()
                    .map(|(name, value)| (name.to_uppercase(), value))
                    .collect();
                (inode, map)
            })
            .collect(),
        Err(e) => {
            log_error(format!(
                "[COMPUTE] DeriveCorpusDeployStatus: get_tags_batch_for_zone failed: {} (will fall back to per-inode queries)",
                e,
            ));
            HashMap::new()
        }
    };

    log_general(format!(
        "[COMPUTE] DeriveCorpusDeployStatus: {} corpus inodes, {} healthy inodes, {} preloaded tag maps, {} source_dirs: [{}]",
        all_corpus_inodes.len(),
        healthy_inodes.len(),
        corpus_tag_maps.len(),
        config.source_dirs.len(),
        config.source_dirs.iter().map(|sd| format!("{:?}(libs={:?})", sd.path, sd.libraries)).collect::<Vec<_>>().join(", "),
    ));

    let mut precomputed: Vec<PrecomputedFile> = Vec::new();
    let mut deploy_path_counts: HashMap<String, usize> = HashMap::new();
    let mut skipped_not_configured = 0usize;
    let mut skipped_no_tags = 0usize;
    let mut skipped_not_healthy = 0usize;
    let mut sample_not_in_source: Option<String> = None;

    for (&inode, corpus_path) in &all_corpus_inodes {
        let tag_map = match corpus_tag_maps.get(&inode) {
            Some(m) => m.clone(),
            None => {
                // Fallback for inodes the batch missed (shouldn't happen, but
                // keeps behavior identical if the batch returns a partial set).
                let tags = read_only_db
                    .get_tags::<crate::zones::CorpusZone>(inode)
                    .unwrap_or_default();
                crate::meta::computations::helpers::tags_to_map(tags)
            }
        };
        if tag_map.is_empty() {
            skipped_no_tags += 1;
            continue;
        }

        let expected_relative = compute_deployment_path_with_tags(corpus_path, &tag_map);
        let deploy_path = expected_relative.to_string_lossy().to_string();

        // Only build precomputed entries for healthy, source-configured files.
        let is_healthy = healthy_inodes.contains(&inode);
        let resolved = config.resolve_source_config(Path::new(corpus_path));
        let in_source = resolved.is_some();
        let library_name = resolved
            .and_then(|r| r.libraries.into_iter().next())
            .unwrap_or_default();
        let keep = is_healthy && in_source;

        if !is_healthy {
            skipped_not_healthy += 1;
        }
        if is_healthy && !in_source && sample_not_in_source.is_none() {
            sample_not_in_source = Some(corpus_path.clone());
        }

        if keep {
            // Move into PrecomputedFile, then count via reference to stored string.
            precomputed.push(PrecomputedFile {
                inode,
                corpus_path: corpus_path.clone(),
                deploy_path,
                library_name,
            });
            let stored = &precomputed.last().unwrap().deploy_path;
            // Entry API needs owned key on first insert; borrow-check on existing.
            if let Some(count) = deploy_path_counts.get_mut(stored.as_str()) {
                *count += 1;
            } else {
                deploy_path_counts.insert(stored.clone(), 1);
            }
        } else {
            // Move directly into counts map — no clone needed.
            *deploy_path_counts.entry(deploy_path).or_insert(0) += 1;
            if healthy_inodes.contains(&inode) {
                skipped_not_configured += 1;
            }
        }
    }

    log_general(format!(
        "[COMPUTE] DeriveCorpusDeployStatus filter: {} precomputed, {} skipped_no_tags, {} skipped_not_healthy, {} skipped_not_configured{}",
        precomputed.len(),
        skipped_no_tags,
        skipped_not_healthy,
        skipped_not_configured,
        sample_not_in_source.as_ref().map(|p| format!(", sample_not_in_source={}", p)).unwrap_or_default(),
    ));

    // Phase 2: Build conflict set — deploy paths claimed by 2+ corpus files.
    let conflict_paths: HashSet<&str> = deploy_path_counts
        .iter()
        .filter(|(_, count)| **count > 1)
        .map(|(path, _)| path.as_str())
        .collect();

    // Phase 2c: For each conflict path, pick the tiebreak winner (first alphabetical corpus_path).
    // Winners get DeployReady; losers are blocked by the DeployConflict signal.
    let conflict_winners: HashSet<i64> = {
        let mut path_groups: HashMap<&str, Vec<&PrecomputedFile>> = HashMap::new();
        for file in &precomputed {
            if conflict_paths.contains(file.deploy_path.as_str()) {
                path_groups
                    .entry(file.deploy_path.as_str())
                    .or_default()
                    .push(file);
            }
        }
        path_groups
            .values()
            .filter_map(|group| group.iter().min_by_key(|f| &f.corpus_path))
            .map(|winner| winner.inode)
            .collect()
    };

    // Phase 2b: Build release overlap set — album directories with overlapping releases.
    // Files targeting these album dirs should not be DeployReady.
    let overlap_album_dirs: HashSet<String> = read_only_db
        .aggregate_signal_keys::<ReleaseOverlapSignal>()
        .unwrap_or_default()
        .into_iter()
        .collect();

    // Phase 3: Build computed signal sets for reconciliation.
    let mut computed_deploy_ready: Vec<ComputedCorpusSignal> = Vec::new();
    let mut computed_deployed_healthy: Vec<ComputedCorpusSignal> = Vec::new();
    let mut deployed_stale_count = 0usize;
    let mut conflict_skipped_count = 0usize;
    let mut overlap_skipped_count = 0usize;
    let mut superseded_count = 0usize;

    for file in &precomputed {
        let phase = classify_corpus_file(file, &library_inode_to_paths);

        match phase {
            DeployLifecyclePhase::Healthy => {
                // If this file is deployed but lost the conflict tiebreak,
                // its deployment is superseded — a different corpus file now
                // owns this deploy path. Don't emit DeployedHealthy; the
                // auto-deploy pipeline will stash the old link before
                // deploying the winner.
                if conflict_paths.contains(file.deploy_path.as_str())
                    && !conflict_winners.contains(&file.inode)
                {
                    superseded_count += 1;
                    log_general(format!(
                        "[COMPUTE] DeriveCorpusDeployStatus: superseded deployment: inode {} at {} (conflict loser for {})",
                        file.inode, file.corpus_path, file.deploy_path,
                    ));
                    continue;
                }

                // Correctly deployed — find the matching library path for the signal
                let lib_path = library_inode_to_paths
                    .get(&file.inode)
                    .and_then(|paths| {
                        paths.iter().find(|lp| {
                            let components: Vec<_> = lp.components().collect();
                            if components.len() > 1 {
                                let suffix: PathBuf = components[1..].iter().collect();
                                suffix == Path::new(&file.deploy_path)
                            } else {
                                false
                            }
                        })
                    })
                    .expect("Healthy implies matching library path");
                let signal = TypedSignalWrite::DeployedHealthy(DeployedHealthySignal {
                    inode: file.inode,
                    path: file.corpus_path.clone(),
                    library_path: lib_path.to_string_lossy().to_string(),
                });
                computed_deployed_healthy.push(ComputedCorpusSignal::new(file.inode, signal));
            }
            DeployLifecyclePhase::Stale => {
                // Deployed but at wrong path — DeriveDeployHealthSignals handles
                // the library-side LibraryStale signal; don't also emit DeployReady.
                deployed_stale_count += 1;
            }
            DeployLifecyclePhase::Ready => {
                // Not deployed — apply conflict/overlap filters before emitting
                if conflict_paths.contains(file.deploy_path.as_str()) {
                    if conflict_winners.contains(&file.inode) {
                        let signal = TypedSignalWrite::DeployReady(DeployReadySignal {
                            inode: file.inode,
                            path: file.corpus_path.clone(),
                            deploy_path: file.deploy_path.clone(),
                            library_name: file.library_name.clone(),
                        });
                        computed_deploy_ready.push(ComputedCorpusSignal::new(file.inode, signal));
                    } else {
                        conflict_skipped_count += 1;
                    }
                } else if overlap_album_dirs.contains(&deploy_album_directory(&file.deploy_path)) {
                    overlap_skipped_count += 1;
                } else {
                    let signal = TypedSignalWrite::DeployReady(DeployReadySignal {
                        inode: file.inode,
                        path: file.corpus_path.clone(),
                        deploy_path: file.deploy_path.clone(),
                        library_name: file.library_name.clone(),
                    });
                    computed_deploy_ready.push(ComputedCorpusSignal::new(file.inode, signal));
                }
            }
            DeployLifecyclePhase::Leftover => {
                // Corpus files can't be leftovers — that's a library-side phase.
                unreachable!("Corpus file cannot be in Leftover phase");
            }
        }
    }

    let deploy_ready_count = computed_deploy_ready.len();
    let deployed_healthy_count = computed_deployed_healthy.len();

    // Signal-flip detection: identify inodes losing DeployedHealthy status.
    // This catches the scenario where UpdateDeploySignals wrote DeployedHealthy
    // but this bulk recomputation is about to clear it (likely a bug or race).
    let existing_deployed_healthy: HashSet<i64> = read_only_db
        .corpus_signal_all_inodes::<DeployedHealthySignal>()
        .unwrap_or_default()
        .into_iter()
        .collect();
    let newly_deployed_healthy: HashSet<i64> =
        computed_deployed_healthy.iter().map(|s| s.inode).collect();
    let newly_deploy_ready: HashSet<i64> = computed_deploy_ready.iter().map(|s| s.inode).collect();
    for &inode in &existing_deployed_healthy {
        if !newly_deployed_healthy.contains(&inode) {
            let reason = if newly_deploy_ready.contains(&inode) {
                "reclassified as DeployReady (inode not found in library files, or path mismatch)"
            } else {
                "dropped entirely (not in configured sources, or not healthy)"
            };
            // Find the path from precomputed if available
            let path = precomputed
                .iter()
                .find(|f| f.inode == inode)
                .map(|f| f.corpus_path.as_str())
                .unwrap_or("<unknown>");
            log_error(format!(
                "[COMPUTE] DeriveCorpusDeployStatus: inode {} was DeployedHealthy, now {} — {}",
                inode, reason, path,
            ));
        }
    }

    let dr_stats = reconcile_corpus_signals::<DeployReadySignal>(
        read_only_db,
        &sender,
        computed_deploy_ready,
        witness,
    );
    let dh_stats = reconcile_corpus_signals::<DeployedHealthySignal>(
        read_only_db,
        &sender,
        computed_deployed_healthy,
        witness,
    );

    // ====================================================================
    // Phase 4: Sidecar image deployment discovery
    // ====================================================================
    //
    // For every corpus directory that has deployed or deploy-ready audio,
    // check if its sidecar images are present in the library. Emit
    // SidecarDeployReady signals for missing images.

    let sidecar_count = derive_sidecar_deploy_signals(
        &precomputed,
        &library_inode_to_paths,
        config,
        read_only_db,
        &sender,
        witness,
    );

    log_general(format!(
        "[COMPUTE] DeriveCorpusDeployStatus: {} total corpus inodes, {} healthy, {} precomputed, {} conflict paths, {} overlap dirs, {} deploy-ready, {} deployed-healthy, {} deployed-stale, {} conflict-skipped, {} overlap-skipped, {} superseded, {} not configured, {} sidecars",
        all_corpus_inodes.len(),
        healthy_signals.len(),
        precomputed.len(),
        conflict_paths.len(),
        overlap_album_dirs.len(),
        deploy_ready_count,
        deployed_healthy_count,
        deployed_stale_count,
        conflict_skipped_count,
        overlap_skipped_count,
        superseded_count,
        skipped_not_configured,
        sidecar_count,
    ));
    log_general(format!(
        "[COMPUTE] DeriveCorpusDeployStatus reconcile: DeployReady({}) DeployedHealthy({})",
        dr_stats, dh_stats,
    ));

    // Clear all dirty marks now that the full scan has run; the next cycle will
    // short-circuit until another TAGS|FILES|DEPLOY mutation marks inodes dirty.
    sender.clear_all_dirty_inodes(
        crate::meta::computations::CORPUS_DEPLOY_STATUS_COMPUTATION,
        witness,
    );

    Result::success(computation, Vec::new())
}

// ============================================================================
// Sidecar Image Deploy Discovery
// ============================================================================

const SIDECAR_DEPLOY_COMPUTATION: &str = "sidecar_deploy";

/// Discover sidecar images that should be deployed alongside audio files.
///
/// For each corpus directory with deploy-ready or deployed-healthy audio,
/// checks sidecar images against library state and emits SidecarDeployReady
/// signals for missing ones.
///
/// Uses a single batch query for all corpus images (instead of per-directory
/// LIKE scans) and dirty-inode tracking to skip or run incrementally on
/// subsequent cycles when nothing sidecar-relevant changed.
///
/// Returns the total number of sidecar signals emitted.
fn derive_sidecar_deploy_signals(
    precomputed: &[PrecomputedFile],
    library_inode_to_paths: &HashMap<i64, Vec<PathBuf>>,
    config: &crate::config::Config,
    read_only_db: &ReadOnlyDb<'_>,
    sender: &write_thread::SignalWriteSender,
    witness: &ComputationWitness,
) -> usize {
    use crate::config::SidecarDeployMode;
    use crate::db::queries::files::CorpusImageEntry;

    let mode = config.opinions.album_art.sidecar_deploy_mode;
    if mode == SidecarDeployMode::Disabled {
        // Clear any existing sidecar signals and return
        let stats = reconcile_corpus_signals::<SidecarDeployReadySignal>(
            read_only_db,
            sender,
            Vec::new(),
            witness,
        );
        if stats.cleared > 0 {
            log_general(format!(
                "[COMPUTE] Sidecar deploy disabled, cleared {} stale signals",
                stats.cleared,
            ));
        }
        return 0;
    }

    // Collect unique corpus directories from precomputed files, with their
    // library_name and album_dir (deploy path parent).
    // dir_targets: corpus_dir → (library_name, album_dir)
    let mut dir_targets: HashMap<String, (String, String)> = HashMap::new();

    for file in precomputed {
        let corpus_dir = Path::new(&file.corpus_path)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();

        if dir_targets.contains_key(&corpus_dir) {
            continue;
        }

        // Resolve library name for this file's source directory
        let library_name = config
            .resolve_source_config_for_db_path(&file.corpus_path)
            .and_then(|r| r.libraries.into_iter().next());

        if let Some(lib) = library_name {
            let album_dir = deploy_album_directory(&file.deploy_path);
            dir_targets.insert(corpus_dir, (lib, album_dir));
        }
    }

    // --- Batch query: 1 query instead of N per-directory LIKE scans ---
    let all_images = read_only_db.get_all_corpus_images().unwrap_or_default();

    // Group images by parent directory
    let mut images_by_dir: HashMap<&str, Vec<&CorpusImageEntry>> = HashMap::new();
    for img in &all_images {
        if let Some(parent) = Path::new(&img.path).parent() {
            let parent_str = parent.to_str().unwrap_or("");
            if dir_targets.contains_key(parent_str) {
                images_by_dir.entry(parent_str).or_default().push(img);
            }
        }
    }

    // --- Dirty-inode gate: skip entirely if nothing changed ---
    let dirty_inodes = read_only_db
        .get_dirty_inodes(SIDECAR_DEPLOY_COMPUTATION)
        .unwrap_or_default();

    let existing_signal_count = read_only_db.corpus_signal_count::<SidecarDeployReadySignal>();

    if dirty_inodes.is_empty() && existing_signal_count > 0 {
        // Nothing changed, existing signals are fresh — skip entirely
        log_general(format!(
            "[COMPUTE] DeriveCorpusDeployStatus sidecar: no dirty inodes, {} existing signals fresh, skipping",
            existing_signal_count,
        ));
        return existing_signal_count;
    }

    // Build set of inodes already present in the library. Image files
    // deployed via hard-link share their inode with the corpus source,
    // so an inode appearing in library_inode_to_paths means it's deployed.
    let library_inodes: HashSet<i64> = library_inode_to_paths.keys().copied().collect();

    // Also build a set of all occupied library paths. The inode check alone
    // misses cases where a DIFFERENT corpus image (different inode) was
    // previously deployed to the same destination path — e.g., when two
    // corpus directories share the same album directory in the library.
    let library_paths: HashSet<PathBuf> = library_inode_to_paths
        .values()
        .flat_map(|paths| paths.iter().cloned())
        .collect();

    // --- Phase 1: Collect candidates by (library_name, album_dir, role_class) ---
    //
    // Cover front and cover back images compete by ROLE within an album: per-disc
    // covers all target the same library album dir, so they must dedupe to a
    // single deploy. Other sidecars (booklet.pdf, liner_notes.txt) deploy with
    // their own filenames and only conflict when two corpus dirs ship the exact
    // same filename.
    type DeployKey = (String, String, SidecarRoleClass);
    let mut candidates_by_deploy: HashMap<DeployKey, Vec<&CorpusImageEntry>> = HashMap::new();

    for (corpus_dir, (library_name, album_dir)) in &dir_targets {
        let images = images_by_dir
            .get(corpus_dir.as_str())
            .map(|v| v.as_slice())
            .unwrap_or(&[]);

        for img in images {
            if let Some((role_class, _filename)) = sidecar_role_class(img, mode) {
                let key = (library_name.clone(), album_dir.clone(), role_class);
                candidates_by_deploy.entry(key).or_default().push(img);
            }
        }
    }

    // --- Phase 2: Classify — deployed, single candidate, or conflict ---
    //
    // Each group is one library destination slot. We dedupe by inode (so phantom
    // inode_paths aliases — e.g., a hardlink broken on disk but still in the DB —
    // don't multi-count), pick the alphabetically-first path per inode for a
    // canonical winner, then classify the slot.
    let mut computed_sidecars: Vec<ComputedCorpusSignal> = Vec::new();
    let mut computed_conflicts: Vec<ComputedAggregateSignal> = Vec::new();

    for ((library_name, album_dir, _role_class), group) in &candidates_by_deploy {
        let mut by_inode: HashMap<i64, &CorpusImageEntry> = HashMap::new();
        for img in group {
            by_inode
                .entry(img.inode)
                .and_modify(|existing: &mut &CorpusImageEntry| {
                    if img.path < existing.path {
                        *existing = *img;
                    }
                })
                .or_insert(img);
        }
        let mut deduped: Vec<&CorpusImageEntry> = by_inode.into_values().collect();
        deduped.sort_by(|a, b| a.path.cmp(&b.path));

        let winner = deduped[0];
        let winner_filename = Path::new(&winner.path)
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_default();
        let deploy_path = format!("{}/{}", album_dir, winner_filename);
        let target_path = Path::new(library_name).join(&deploy_path);

        let any_deployed = deduped.iter().any(|img| library_inodes.contains(&img.inode));
        let path_blocked = library_paths.contains(&target_path);

        // Only the canonical winner is ever offered for deploy. Phantom alias
        // paths (Cover2.png, Cover3.png from broken hardlinks) no longer leak
        // into SidecarDeployReady because they share the role-class slot.
        if !any_deployed && !path_blocked {
            computed_sidecars.push(make_sidecar_ready_signal(winner, &deploy_path, library_name));
        }

        // Emit Conflict whenever 2+ distinct inodes compete — regardless of
        // whether one is already deployed. The operator sees the full competing
        // set so they can pick a winner explicitly.
        if deduped.len() >= 2 {
            let conflict_key = format!("{}/{}", library_name, deploy_path);
            let inodes: Vec<i64> = deduped.iter().map(|img| img.inode).collect();
            let signal = TypedSignalWrite::SidecarDeployConflict(SidecarDeployConflictSignal {
                key: conflict_key.clone(),
                deploy_path: deploy_path.clone(),
                library_name: library_name.clone(),
                inodes,
            });
            computed_conflicts.push(ComputedAggregateSignal::new(conflict_key, signal));
        }
    }

    // --- Phase 3: Reconcile globally ---
    // Conflict detection requires cross-directory awareness (images from different
    // corpus directories can target the same deploy path), so we always do full
    // reconciliation. The dirty-inode gate at the top already skips this entirely
    // when nothing has changed.
    let sidecar_count = computed_sidecars.len();

    let sc_stats = reconcile_corpus_signals::<SidecarDeployReadySignal>(
        read_only_db,
        sender,
        computed_sidecars,
        witness,
    );

    let cc_stats = reconcile_aggregate_signals::<SidecarDeployConflictSignal>(
        read_only_db,
        sender,
        computed_conflicts,
        witness,
    );

    log_general(format!(
        "[COMPUTE] DeriveCorpusDeployStatus sidecar reconcile: {} dirs, {} images, \
         SidecarDeployReady({}), SidecarDeployConflict({})",
        dir_targets.len(),
        all_images.len(),
        sc_stats,
        cc_stats,
    ));

    // Clear all dirty inodes after recompute
    for inode in &dirty_inodes {
        sender.clear_dirty_inode(*inode, SIDECAR_DEPLOY_COMPUTATION, witness);
    }

    sidecar_count
}

/// Slot grouping for sidecar deploy candidates within an album dir.
///
/// `cover_front` / `cover_back` images all compete for ONE slot per album,
/// regardless of source filename — multi-disc albums commonly ship the same
/// cover under each disc, but only one ends up in the library. Other sidecars
/// (booklet.pdf, liner_notes.txt, ...) keep their filename as the slot
/// discriminator and only conflict on exact filename collision.
#[derive(Hash, Eq, PartialEq, Clone, Debug)]
enum SidecarRoleClass {
    PrimaryCover,
    BackCover,
    Named(String),
}

/// Classify a corpus image into a role-based deploy slot.
///
/// Returns `Some((role_class, filename))` if the image passes the mode filter,
/// `None` otherwise. The filename is returned for downstream deploy_path
/// construction (the winner of a contested slot deploys with its own filename).
fn sidecar_role_class(
    img: &crate::db::queries::files::CorpusImageEntry,
    mode: crate::config::SidecarDeployMode,
) -> Option<(SidecarRoleClass, String)> {
    use crate::config::SidecarDeployMode;

    match mode {
        SidecarDeployMode::PrimaryCover => {
            if img.role != "cover_front" {
                return None;
            }
        }
        SidecarDeployMode::All => {}
        SidecarDeployMode::Disabled => unreachable!(),
    }

    let filename = Path::new(&img.path)
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_default();

    if filename.is_empty() {
        return None;
    }

    let role_class = match img.role.as_str() {
        "cover_front" => SidecarRoleClass::PrimaryCover,
        "cover_back" => SidecarRoleClass::BackCover,
        _ => SidecarRoleClass::Named(filename.clone()),
    };

    Some((role_class, filename))
}

/// Build a SidecarDeployReady corpus signal for the given image.
fn make_sidecar_ready_signal(
    img: &crate::db::queries::files::CorpusImageEntry,
    deploy_path: &str,
    library_name: &str,
) -> ComputedCorpusSignal {
    let signal = TypedSignalWrite::SidecarDeployReady(SidecarDeployReadySignal {
        inode: img.inode,
        path: img.path.clone(),
        deploy_path: deploy_path.to_string(),
        library_name: library_name.to_string(),
        data: SidecarDeployReadyData {
            role: img.role.clone(),
            format: img.format.clone(),
            width: img.width,
            height: img.height,
        },
    });
    ComputedCorpusSignal::new(img.inode, signal)
}
