//! Deploy-related executors.
//!
//! Deploy conflict detection, deploy health signals, and corpus deploy status.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::logging::{log_general, log_error};
use crate::meta::computations::types::ComputationWitness;
use crate::meta::computations::helpers::{ComputedAggregateSignal, ComputedCorpusSignal, reconcile_aggregate_signals, reconcile_corpus_signals};
use crate::meta::computations::helpers::is_image_file;
use crate::meta::signals::data::{
    TypedSignalWrite, DeployConflictSignal, DeployReadySignal, DeployedHealthySignal,
    DeployLifecyclePhase,
    LibraryLeftoverSignal, LibraryStaleSignal,
    ReleaseOverlapSignal, ReleaseOverlapData, ReleaseOverlapEntry,
    SidecarDeployReadySignal, SidecarDeployReadyData,
    SidecarDeployConflictSignal,
};
use crate::corpus::deploy::{compute_deployment_path_with_tags, deploy_album_directory, extract_release_directory};
use crate::db::ReadOnlyDb;
use crate::db::write_thread;

use super::{Computation, Result};

/// A corpus file with its precomputed deploy path and library association.
///
/// Used by `DeriveCorpusDeployStatus` to batch-process deploy status, and by
/// `derive_sidecar_deploy_signals` to discover directories needing sidecars.
struct PrecomputedFile {
    inode: i64,
    corpus_path: String,
    deploy_path: String,
}

// ============================================================================
// Deploy Conflict Detection
// ============================================================================

/// Execute DetectDeployConflicts - bulk detection of deploy path collisions.
pub fn execute_detect_deploy_conflicts(
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    let computation = Computation::DetectDeployConflicts;

    let sender = match write_thread::signal_sender() {
        Some(s) => s.clone(),
        None => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                "DB thread not initialized".to_string(),
            );
        }
    };

    let healthy_signals = match read_only_db.get_healthy_file_signals() {
        Ok(v) => v,
        Err(e) => {
            log_error(format!(
                "[COMPUTE] DetectDeployConflicts: get_healthy_file_signals failed: {}", e
            ));
            return Result::failure(computation, start.elapsed().as_millis() as u64, format!("get_healthy_file_signals: {}", e));
        }
    };

    let mut deploy_path_to_tracks: HashMap<String, Vec<i64>> = HashMap::new();

    for signal in &healthy_signals {
        let tags = match read_only_db.get_corpus_tags(signal.inode) {
            Ok(v) => v,
            Err(e) => {
                log_error(format!(
                    "[COMPUTE] DetectDeployConflicts: get_corpus_tags failed for inode {}: {}", signal.inode, e
                ));
                Vec::new()
            }
        };
        let tag_map: HashMap<String, String> = tags
            .into_iter()
            .map(|t| (t.tag_name.to_uppercase(), t.tag_value))
            .collect();

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
                deploy_path: deploy_path.clone(),
                inodes: inodes.clone(),
            });
            computed.push(ComputedAggregateSignal::new(deploy_path.clone(), signal));
        }
    }

    let conflict_count = computed.len();
    let (cleared, new, updated, unchanged) = reconcile_aggregate_signals::<DeployConflictSignal>(
        read_only_db,
        &sender,
        computed,
        witness,
    );

    log_general(format!(
        "[COMPUTE] DetectDeployConflicts: {} conflicts among {} healthy files (reconcile: {} cleared, {} new, {} updated, {} unchanged)",
        conflict_count,
        healthy_signals.len(),
        cleared,
        new,
        updated,
        unchanged,
    ));

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
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
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    let computation = Computation::DetectReleaseOverlaps;

    let sender = match write_thread::signal_sender() {
        Some(s) => s.clone(),
        None => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                "DB thread not initialized".to_string(),
            );
        }
    };

    let config = match crate::config::load_config() {
        Ok(c) => c,
        Err(_) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                "Failed to load config".to_string(),
            );
        }
    };

    let healthy_signals = match read_only_db.get_healthy_file_signals() {
        Ok(v) => v,
        Err(e) => {
            log_error(format!(
                "[COMPUTE] DetectReleaseOverlaps: get_healthy_file_signals failed: {}", e
            ));
            return Result::failure(computation, start.elapsed().as_millis() as u64, format!("get_healthy_file_signals: {}", e));
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
        // Strip "corpus/" prefix for source directory lookup (signal paths are corpus-prefixed)
        let relative_path = signal.path.strip_prefix("corpus/").unwrap_or(&signal.path);

        let resolved = match config.resolve_source_config(Path::new(relative_path)) {
            Some(r) => r,
            None => continue,
        };

        let tags = match read_only_db.get_corpus_tags(signal.inode) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if tags.is_empty() {
            continue;
        }

        let tag_map: HashMap<String, String> = tags
            .into_iter()
            .map(|t| (t.tag_name.to_uppercase(), t.tag_value))
            .collect();

        let deploy_path = compute_deployment_path_with_tags(&signal.path, &tag_map)
            .to_string_lossy()
            .to_string();

        let album_dir = deploy_album_directory(&deploy_path);
        if album_dir.is_empty() || album_dir.starts_with("[no album artist]") {
            continue;
        }

        let source_dir = resolved.source_path.to_string_lossy().to_string();
        let release_dir = extract_release_directory(relative_path, &resolved.source_path);

        album_dir_files.entry(album_dir).or_default().push(FileInfo {
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
            let corpus_paths: Vec<String> = group_files.iter().map(|f| f.corpus_path.clone()).collect();
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
        releases.sort_by(|a, b| (&a.source_dir, &a.release_dir).cmp(&(&b.source_dir, &b.release_dir)));

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
    let (cleared, new, updated, unchanged) = reconcile_aggregate_signals::<ReleaseOverlapSignal>(
        read_only_db,
        &sender,
        computed,
        witness,
    );

    log_general(format!(
        "[COMPUTE] DetectReleaseOverlaps: {} overlapping album dirs among {} dirs ({} healthy files) (reconcile: {} cleared, {} new, {} updated, {} unchanged)",
        overlap_count,
        album_dir_files.len(),
        healthy_signals.len(),
        cleared,
        new,
        updated,
        unchanged,
    ));

    // DeriveCorpusDeployStatus reads ReleaseOverlap signals written above.
    // Defer it behind a barrier so the Witch drains all in-flight work + DB
    // writes before spawning it.
    Result::pipeline(
        computation,
        start.elapsed().as_millis() as u64,
        vec![],
        vec![(
            super::super::PipelineStage::DependentAnalysis,
            vec![
                super::super::Computation::Analysis(Computation::DeriveCorpusDeployStatus),
            ],
        )],
    )
}

// ============================================================================
// Deploy Health Signals
// ============================================================================

/// Execute DeriveDeployHealthSignals - derive library health from scan data.
///
/// Reads library file data from files table (source='library') and compares against
/// corpus index to identify leftovers and stale deployments.
pub fn execute_derive_deploy_health_signals(
    read_only_db: &ReadOnlyDb<'_>,
    library_name: &str,
    library_root: &Path,
    corpus_path_prefixes: &[std::path::PathBuf],
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    let computation = Computation::DeriveDeployHealthSignals {
        library_name: library_name.to_string(),
        library_root: library_root.to_path_buf(),
        corpus_path_prefixes: corpus_path_prefixes.to_vec(),
    };

    let sender = match write_thread::signal_sender() {
        Some(s) => s.clone(),
        None => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                "DB thread not initialized".to_string(),
            );
        }
    };

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
            return Result::success(computation, start.elapsed().as_millis() as u64, Vec::new());
        }
    };

    log_general(format!(
        "[COMPUTE] DeriveDeployHealthSignals '{}': got {} library scan entries from DB",
        library_name, library_scan_entries.len(),
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
    let corpus_inodes = match read_only_db.get_all_corpus_inodes() {
        Ok(v) => v,
        Err(e) => {
            log_error(format!(
                "[COMPUTE] DeriveDeployHealthSignals '{}': get_all_corpus_inodes failed: {}", library_name, e
            ));
            return Result::failure(computation, start.elapsed().as_millis() as u64, format!("get_all_corpus_inodes: {}", e));
        }
    };

    log_general(format!(
        "[COMPUTE] DeriveDeployHealthSignals '{}': {} corpus inodes loaded",
        library_name, corpus_inodes.len(),
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
                    &library_path_to_inode,
                    &mut dir_to_album_dir,
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
                    &mut dir_to_album_dir,
                );
                if let Some(expected_path) = expected_with_prefix {
                    let is_image = is_image_file(Path::new(corpus_path));
                    let stale_key = LibraryStaleSignal::make_key(library_name, &library_path_display);
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
                let leftover_key = LibraryLeftoverSignal::make_key(library_name, &library_path_display);
                sender.write_typed_signal(
                    TypedSignalWrite::LibraryLeftover(LibraryLeftoverSignal {
                        key: leftover_key,
                    }),
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

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
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
    library_path_to_inode: &HashMap<PathBuf, i64>,
    dir_to_album_dir: &mut HashMap<String, Option<String>>,
) -> DeployLifecyclePhase {
    // Try audio file path first
    if let Ok(Some(audio_file)) = read_only_db.get_audio_file_by_path(corpus_path) {
        return classify_audio_stale(
            read_only_db,
            library_name,
            library_path,
            library_inode,
            corpus_path,
            audio_file.inode(),
            library_path_to_inode,
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
    )
}

/// Classify an audio library file as stale or healthy by comparing deploy paths.
fn classify_audio_stale(
    read_only_db: &ReadOnlyDb<'_>,
    library_name: &str,
    library_path: &Path,
    library_inode: i64,
    corpus_path: &str,
    inode: i64,
    library_path_to_inode: &HashMap<PathBuf, i64>,
) -> DeployLifecyclePhase {
    let tags = match read_only_db.get_corpus_tags(inode) {
        Ok(v) => v,
        Err(e) => {
            log_error(format!(
                "[COMPUTE] DeriveDeployHealthSignals: get_corpus_tags failed for inode {} (corpus={}): {}",
                inode, corpus_path, e
            ));
            Vec::new()
        }
    };
    let tag_map: HashMap<String, String> = tags
        .into_iter()
        .map(|t| (t.tag_name.to_uppercase(), t.tag_value))
        .collect();

    let expected_relative = compute_deployment_path_with_tags(corpus_path, &tag_map);
    let library_path_suffix = library_path
        .strip_prefix(library_name)
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|_| library_path.to_path_buf());

    if library_path_suffix == expected_relative {
        return DeployLifecyclePhase::Healthy;
    }

    // Path mismatch — check for stale-conflict
    let expected_with_prefix = Path::new(library_name).join(&expected_relative);
    if let Some(&occupant_inode) = library_path_to_inode.get(&expected_with_prefix) {
        if occupant_inode != library_inode {
            return DeployLifecyclePhase::Healthy; // Stale-conflict: mask as healthy
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
) -> DeployLifecyclePhase {
    let corpus_dir = Path::new(corpus_path)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    // Lazily populate album_dir for this corpus directory
    let album_dir = dir_to_album_dir
        .entry(corpus_dir.clone())
        .or_insert_with(|| {
            lookup_album_dir_from_sibling(read_only_db, &corpus_dir)
        })
        .clone();

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

    // Path mismatch — check for stale-conflict
    let expected_with_prefix = Path::new(library_name).join(&expected_relative);
    if let Some(&occupant_inode) = library_path_to_inode.get(&expected_with_prefix) {
        if occupant_inode != library_inode {
            return DeployLifecyclePhase::Healthy; // Stale-conflict: mask as healthy
        }
    }

    DeployLifecyclePhase::Stale
}

/// Look up the deploy album directory for a corpus directory by finding an audio
/// sibling and computing its deploy path from tags.
fn lookup_album_dir_from_sibling(
    read_only_db: &ReadOnlyDb<'_>,
    corpus_dir: &str,
) -> Option<String> {
    let (sibling_inode, sibling_path) = read_only_db
        .get_any_audio_sibling_in_directory(corpus_dir)
        .ok()??;

    let tags = read_only_db.get_corpus_tags(sibling_inode).ok()?;
    if tags.is_empty() {
        return None;
    }

    let tag_map: HashMap<String, String> = tags
        .into_iter()
        .map(|t| (t.tag_name.to_uppercase(), t.tag_value))
        .collect();

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
    dir_to_album_dir: &mut HashMap<String, Option<String>>,
) -> Option<String> {
    // Audio file: compute from tags directly
    if let Ok(Some(_audio_file)) = read_only_db.get_audio_file_by_path(corpus_path) {
        let tags = read_only_db.get_corpus_tags(library_inode).ok()?;
        let tag_map: HashMap<String, String> = tags
            .into_iter()
            .map(|t| (t.tag_name.to_uppercase(), t.tag_value))
            .collect();
        let expected_relative = compute_deployment_path_with_tags(corpus_path, &tag_map);
        let expected_with_prefix = Path::new(library_name).join(&expected_relative);
        return Some(expected_with_prefix.to_string_lossy().to_string());
    }

    // Image file: derive from sibling album_dir
    let corpus_dir = Path::new(corpus_path)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    let album_dir = dir_to_album_dir
        .entry(corpus_dir.clone())
        .or_insert_with(|| lookup_album_dir_from_sibling(read_only_db, &corpus_dir))
        .clone()?;

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
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    let computation = Computation::DeriveCorpusDeployStatus;

    let sender = match write_thread::signal_sender() {
        Some(s) => s.clone(),
        None => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                "DB thread not initialized".to_string(),
            );
        }
    };

    // Get all HealthyFile signals
    let healthy_signals = match read_only_db.get_healthy_file_signals() {
        Ok(v) => v,
        Err(e) => {
            log_error(format!(
                "[COMPUTE] DeriveCorpusDeployStatus: get_healthy_file_signals failed: {}", e
            ));
            return Result::failure(computation, start.elapsed().as_millis() as u64, format!("get_healthy_file_signals: {}", e));
        }
    };

    // Build inode → library paths map from files table (source='library')
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
    let config = match crate::config::load_config() {
        Ok(c) => c,
        Err(_) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                "Failed to load config".to_string(),
            );
        }
    };

    // Phase 1: Compute deploy paths for ALL indexed corpus files to build
    // the collision map.  A file cannot be DeployReady if ANY other corpus
    // inode computes the same deploy path — regardless of health status.
    // Only healthy, source-configured files go into `precomputed` for signal
    // emission, but deploy_path_counts covers the entire corpus.

    let all_corpus_inodes = match read_only_db.get_all_corpus_inodes() {
        Ok(v) => v,
        Err(e) => {
            log_error(format!(
                "[COMPUTE] DeriveCorpusDeployStatus: get_all_corpus_inodes failed: {}", e
            ));
            return Result::failure(computation, start.elapsed().as_millis() as u64, format!("get_all_corpus_inodes: {}", e));
        }
    };

    let healthy_inodes: HashSet<i64> = healthy_signals.iter().map(|s| s.inode).collect();

    let mut precomputed: Vec<PrecomputedFile> = Vec::new();
    let mut deploy_path_counts: HashMap<String, usize> = HashMap::new();
    let mut skipped_not_configured = 0usize;

    for (&inode, corpus_path) in &all_corpus_inodes {
        let tags = match read_only_db.get_corpus_tags(inode) {
            Ok(v) => v,
            Err(e) => {
                log_error(format!(
                    "[COMPUTE] DeriveCorpusDeployStatus: get_corpus_tags failed for inode {} (path={}): {}",
                    inode, corpus_path, e
                ));
                Vec::new()
            }
        };
        if tags.is_empty() {
            continue;
        }

        let tag_map: HashMap<String, String> = tags
            .into_iter()
            .map(|t| (t.tag_name.to_uppercase(), t.tag_value))
            .collect();
        let expected_relative = compute_deployment_path_with_tags(corpus_path, &tag_map);
        let deploy_path = expected_relative.to_string_lossy().to_string();

        // Count ALL corpus files for conflict detection.
        *deploy_path_counts.entry(deploy_path.clone()).or_insert(0) += 1;

        // Only build precomputed entries for healthy, source-configured files.
        if healthy_inodes.contains(&inode) {
            let in_source = config.is_path_in_source(Path::new(corpus_path));
            if in_source {
                precomputed.push(PrecomputedFile {
                    inode,
                    corpus_path: corpus_path.clone(),
                    deploy_path,
                });
            } else {
                skipped_not_configured += 1;
            }
        }
    }

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
                path_groups.entry(file.deploy_path.as_str()).or_default().push(file);
            }
        }
        path_groups.values()
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

    for file in &precomputed {
        let phase = classify_corpus_file(file, &library_inode_to_paths);

        match phase {
            DeployLifecyclePhase::Healthy => {
                // Correctly deployed — find the matching library path for the signal
                let lib_path = library_inode_to_paths.get(&file.inode)
                    .and_then(|paths| paths.iter().find(|lp| {
                        let components: Vec<_> = lp.components().collect();
                        if components.len() > 1 {
                            let suffix: PathBuf = components[1..].iter().collect();
                            suffix == Path::new(&file.deploy_path)
                        } else {
                            false
                        }
                    }))
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
    let newly_deployed_healthy: HashSet<i64> = computed_deployed_healthy.iter().map(|s| s.inode).collect();
    let newly_deploy_ready: HashSet<i64> = computed_deploy_ready.iter().map(|s| s.inode).collect();
    for &inode in &existing_deployed_healthy {
        if !newly_deployed_healthy.contains(&inode) {
            let reason = if newly_deploy_ready.contains(&inode) {
                "reclassified as DeployReady (inode not found in library files, or path mismatch)"
            } else {
                "dropped entirely (not in configured sources, or not healthy)"
            };
            // Find the path from precomputed if available
            let path = precomputed.iter()
                .find(|f| f.inode == inode)
                .map(|f| f.corpus_path.as_str())
                .unwrap_or("<unknown>");
            log_error(format!(
                "[COMPUTE] DeriveCorpusDeployStatus: inode {} was DeployedHealthy, now {} — {}",
                inode, reason, path,
            ));
        }
    }

    let (dr_cleared, dr_new, dr_updated, dr_unchanged) = reconcile_corpus_signals::<DeployReadySignal>(
        read_only_db,
        &sender,
        computed_deploy_ready,
        witness,
    );
    let (dh_cleared, dh_new, dh_updated, dh_unchanged) = reconcile_corpus_signals::<DeployedHealthySignal>(
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
        &config,
        read_only_db,
        &sender,
        witness,
    );

    log_general(format!(
        "[COMPUTE] DeriveCorpusDeployStatus: {} total corpus inodes, {} healthy, {} precomputed, {} conflict paths, {} overlap dirs, {} deploy-ready, {} deployed-healthy, {} deployed-stale, {} conflict-skipped, {} overlap-skipped, {} not configured, {} sidecars",
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
        skipped_not_configured,
        sidecar_count,
    ));
    log_general(format!(
        "[COMPUTE] DeriveCorpusDeployStatus reconcile: DeployReady({} cleared, {} new, {} updated, {} unchanged) DeployedHealthy({} cleared, {} new, {} updated, {} unchanged)",
        dr_cleared, dr_new, dr_updated, dr_unchanged,
        dh_cleared, dh_new, dh_updated, dh_unchanged,
    ));

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
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
        let (cleared, _, _, _) = reconcile_corpus_signals::<SidecarDeployReadySignal>(
            read_only_db,
            sender,
            Vec::new(),
            witness,
        );
        if cleared > 0 {
            log_general(format!(
                "[COMPUTE] Sidecar deploy disabled, cleared {} stale signals", cleared,
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

    let existing_signal_count = read_only_db
        .corpus_signal_count::<SidecarDeployReadySignal>();

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

    // --- Phase 1: Collect all candidate images per deploy key ---
    // Group candidates by (library_name, deploy_path) to detect conflicts.
    // An image is a candidate if it passes mode filter and isn't already deployed.
    type DeployKey = (String, String); // (library_name, deploy_path)
    let mut candidates_by_deploy: HashMap<DeployKey, Vec<&CorpusImageEntry>> = HashMap::new();

    for (_corpus_dir, (library_name, album_dir)) in &dir_targets {
        let images = images_by_dir.get(_corpus_dir.as_str()).map(|v| v.as_slice()).unwrap_or(&[]);

        for img in images {
            if let Some((key, _filename)) = sidecar_candidate_key(img, mode, library_name, album_dir) {
                candidates_by_deploy.entry(key).or_default().push(img);
            }
        }
    }

    // --- Phase 2: Classify — deployed, single candidate, or conflict ---
    let mut computed_sidecars: Vec<ComputedCorpusSignal> = Vec::new();
    let mut computed_conflicts: Vec<ComputedAggregateSignal> = Vec::new();

    for ((library_name, deploy_path), group) in &candidates_by_deploy {
        // If any image in this group is already deployed, the path is satisfied.
        // No SidecarDeployReady needed (would fail with "Destination already exists").
        // Also check if the destination path is already occupied by a different file
        // (e.g., an image from another corpus directory deployed to the same album dir).
        let any_deployed = group.iter().any(|img| library_inodes.contains(&img.inode));
        let path_occupied = library_paths.contains(Path::new(library_name).join(deploy_path).as_path());
        if any_deployed || path_occupied {
            // Still emit conflict signal if 2+ non-deployed images also target this path,
            // so the operator knows about the duplicate corpus images.
            let non_deployed: Vec<&&CorpusImageEntry> = group.iter()
                .filter(|img| !library_inodes.contains(&img.inode))
                .collect();
            if non_deployed.len() >= 2 {
                let conflict_key = format!("{}/{}", library_name, deploy_path);
                let inodes: Vec<i64> = group.iter().map(|img| img.inode).collect();
                let signal = TypedSignalWrite::SidecarDeployConflict(SidecarDeployConflictSignal {
                    key: conflict_key.clone(),
                    deploy_path: deploy_path.clone(),
                    library_name: library_name.clone(),
                    inodes,
                });
                computed_conflicts.push(ComputedAggregateSignal::new(conflict_key, signal));
            }
            continue;
        }

        // No image deployed yet
        if group.len() == 1 {
            // Single candidate: emit SidecarDeployReady
            let img = group[0];
            computed_sidecars.push(make_sidecar_ready_signal(img, deploy_path, library_name));
        } else {
            // Conflict: tiebreak winner (alphabetically first corpus path) gets DeployReady,
            // all inodes go into the conflict signal.
            let winner = group.iter().min_by_key(|img| &img.path).unwrap();
            computed_sidecars.push(make_sidecar_ready_signal(winner, deploy_path, library_name));

            let conflict_key = format!("{}/{}", library_name, deploy_path);
            let inodes: Vec<i64> = group.iter().map(|img| img.inode).collect();
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

    let (sc_cleared, sc_new, sc_updated, sc_unchanged) = reconcile_corpus_signals::<SidecarDeployReadySignal>(
        read_only_db,
        sender,
        computed_sidecars,
        witness,
    );

    let (cc_cleared, cc_new, cc_updated, cc_unchanged) = reconcile_aggregate_signals::<SidecarDeployConflictSignal>(
        read_only_db,
        sender,
        computed_conflicts,
        witness,
    );

    log_general(format!(
        "[COMPUTE] DeriveCorpusDeployStatus sidecar reconcile: {} dirs, {} images, \
         SidecarDeployReady({} cleared, {} new, {} updated, {} unchanged), \
         SidecarDeployConflict({} cleared, {} new, {} updated, {} unchanged)",
        dir_targets.len(), all_images.len(),
        sc_cleared, sc_new, sc_updated, sc_unchanged,
        cc_cleared, cc_new, cc_updated, cc_unchanged,
    ));

    // Clear all dirty inodes after recompute
    for inode in &dirty_inodes {
        sender.clear_dirty_inode(*inode, SIDECAR_DEPLOY_COMPUTATION, witness);
    }

    sidecar_count
}

/// Check if a corpus image is a deployment candidate and return its deploy key.
///
/// Returns `Some((deploy_key, filename))` if the image passes the mode filter
/// and is not already deployed (inode not in library). Returns `None` otherwise.
fn sidecar_candidate_key(
    img: &crate::db::queries::files::CorpusImageEntry,
    mode: crate::config::SidecarDeployMode,
    library_name: &str,
    album_dir: &str,
) -> Option<((String, String), String)> {
    use crate::config::SidecarDeployMode;

    // Apply mode filter
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

    let deploy_path = format!("{}/{}", album_dir, filename);
    let key = (library_name.to_string(), deploy_path);
    Some((key, filename))
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
