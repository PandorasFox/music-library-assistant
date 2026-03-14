//! Modal Data Loaders
//!
//! Free functions for loading modal data from the database.
//! These run server-side within domain query closures, breaking the
//! compile-time coupling between UI types and `ReadOnlyDb`.
//!
//! The UI type files remain pure data structures (no DB imports).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::os::unix::fs::MetadataExt;

use anyhow::Result;

use crate::corpus::paths;
use crate::corpus::tags::{self as tags, TagSet};
use crate::db::types::Zone;
use crate::db::ReadOnlyDb;

// ============================================================================
// Corrupt File Modal
// ============================================================================

/// Load corrupt files from the database.
///
/// Corrupt files exist on disk but failed indexing (tag parse error or audio decode failure).
/// We get the inode from the filesystem directly since the file exists on disk.
pub fn load_corrupt_file_data(
    read_db: &ReadOnlyDb<'_>,
) -> Result<mm_meta::views::health_modals::CorruptFileModalData> {
    use mm_meta::views::health_modals::{CorruptFileEntry, CorruptFileModalData};

    let corrupt_paths = read_db.get_corrupt_file_paths()?;

    if corrupt_paths.is_empty() {
        return Ok(CorruptFileModalData::default());
    }

    let resolver = paths::get_resolver();
    let mut files = Vec::new();

    for corpus_path in corrupt_paths {
        let abs_path = resolver.resolve(std::path::Path::new(&corpus_path));
        let inode = if let Ok(metadata) = std::fs::metadata(&abs_path) {
            metadata.ino() as i64
        } else {
            continue;
        };

        files.push(CorruptFileEntry { corpus_path, inode });
    }

    Ok(CorruptFileModalData { files })
}

// ============================================================================
// Missing File Modal
// ============================================================================

/// Load and categorize missing files from the database.
pub fn load_missing_file_data(
    read_db: &ReadOnlyDb<'_>,
) -> Result<mm_meta::views::health_modals::MissingFileModalData> {
    use mm_meta::views::health_modals::{
        MissingFileModalData, NonRestorableMissingFile, RestorableMissingFile,
    };

    let missing_paths = read_db.get_missing_file_paths()?;

    if missing_paths.is_empty() {
        return Ok(MissingFileModalData::default());
    }

    let library_files = read_db.get_all_library_files()?;
    let inode_to_library: HashMap<i64, String> = library_files
        .into_iter()
        .map(|e| (e.inode, e.file_path.to_string_lossy().to_string()))
        .collect();

    let mut restorable = Vec::new();
    let mut non_restorable = Vec::new();

    for corpus_path in missing_paths {
        let inode = if let Some(af) = read_db.get_audio_file_by_path(&corpus_path)? {
            Some(af.inode())
        } else if let Some(fe) = read_db.get_file_entry_by_path(&corpus_path, "corpus")? {
            Some(fe.inode)
        } else {
            None
        };

        if let Some(inode) = inode {
            if let Some(library_path) = inode_to_library.get(&inode) {
                restorable.push(RestorableMissingFile {
                    corpus_path,
                    library_path: library_path.clone(),
                    inode,
                });
            } else {
                non_restorable.push(NonRestorableMissingFile {
                    corpus_path,
                    inode: Some(inode),
                });
            }
        } else {
            non_restorable.push(NonRestorableMissingFile {
                corpus_path,
                inode: None,
            });
        }
    }

    Ok(MissingFileModalData {
        restorable,
        non_restorable,
    })
}

// ============================================================================
// Missing Directory Modal
// ============================================================================

/// Load missing directory data from the database.
pub fn load_missing_directory_data(
    read_db: &ReadOnlyDb<'_>,
) -> Result<mm_meta::views::health_modals::MissingDirectoryModalData> {
    let directories = read_db.get_missing_directory_paths()?;
    Ok(mm_meta::views::health_modals::MissingDirectoryModalData { directories })
}

// ============================================================================
// Subpar Duplicate Modal
// ============================================================================

/// Load subpar duplicate files from the database.
pub fn load_subpar_duplicate_data(
    read_db: &ReadOnlyDb<'_>,
) -> Result<mm_meta::views::health_modals::SubparDuplicateModalData> {
    use mm_meta::views::health_modals::{SubparDuplicateModalData, SubparFileEntry};

    let subpar_entries = read_db.get_subpar_duplicate_files()?;

    if subpar_entries.is_empty() {
        return Ok(SubparDuplicateModalData::default());
    }

    let resolver = paths::get_resolver();
    let mut files = Vec::new();

    for entry in subpar_entries {
        let inode = if let Some(af) = read_db.get_audio_file_by_path(&entry.corpus_path)? {
            af.inode()
        } else if let Some(fe) = read_db.get_file_entry_by_path(&entry.corpus_path, "corpus")? {
            fe.inode
        } else {
            let abs_path = resolver.resolve(std::path::Path::new(&entry.corpus_path));
            if let Ok(metadata) = std::fs::metadata(&abs_path) {
                metadata.ino() as i64
            } else {
                continue;
            }
        };

        let reason = match entry.reason.as_str() {
            "SubparBitrate" => "Lower bitrate".to_string(),
            "SubparFormat" => "Worse format".to_string(),
            "SubparSampleRate" => "Lower sample rate".to_string(),
            other => other.to_string(),
        };

        files.push(SubparFileEntry {
            corpus_path: entry.corpus_path,
            inode,
            reason,
            superior_path: entry.superior_path,
            similarity_score: entry.similarity_score,
        });
    }

    Ok(SubparDuplicateModalData { files })
}

// ============================================================================
// Shit Format Modal
// ============================================================================

/// Load shit format files from the database.
pub fn load_shit_format_data(
    read_db: &ReadOnlyDb<'_>,
) -> Result<mm_meta::views::cluster_deploy::ShitFormatModalData> {
    use mm_meta::views::cluster_deploy::{ShitFormatEntry, ShitFormatModalData};

    let shit_format_files = read_db.get_shit_format_files()?;
    let file_counts_raw = read_db.get_shit_format_counts_by_type()?;

    if shit_format_files.is_empty() {
        return Ok(ShitFormatModalData::default());
    }

    let mut lossless_files = Vec::new();
    let mut lossy_files = Vec::new();

    for (inode, _signal_path, file_type) in shit_format_files {
        let corpus_path = match read_db.get_audio_file_by_inode(inode, Zone::Corpus)? {
            Some(af) => af.path().to_string(),
            None => continue,
        };

        let entry = ShitFormatEntry {
            corpus_path,
            inode,
            file_type,
        };

        if entry.is_lossless() {
            lossless_files.push(entry);
        } else if entry.is_lossy() {
            lossy_files.push(entry);
        }
    }

    let file_counts: HashMap<String, i64> = file_counts_raw.into_iter().collect();

    Ok(ShitFormatModalData::new_from_loaded(
        lossless_files,
        lossy_files,
        file_counts,
    ))
}

// ============================================================================
// Directory Cluster Modal
// ============================================================================

/// Build a human-readable format summary from format counts.
fn format_summary_from_counts(format_counts: &HashMap<String, usize>) -> String {
    if format_counts.len() == 1 {
        let (fmt, count) = format_counts.iter().next().unwrap();
        format!("{} ({})", fmt, count)
    } else if format_counts.is_empty() {
        "unknown".to_string()
    } else {
        let mut parts: Vec<String> = format_counts
            .iter()
            .map(|(fmt, count)| format!("{} ({})", fmt, count))
            .collect();
        parts.sort();
        parts.join(", ")
    }
}

/// Load cross-source overlap clusters from the database.
pub fn load_directory_cluster_data(
    read_db: &ReadOnlyDb<'_>,
) -> Result<mm_meta::views::cluster_deploy::DirectoryClusterModalData> {
    use mm_meta::views::cluster_deploy::{
        DirectoryClusterEntry, DirectoryClusterModalData, DirectoryGroupEntry,
    };

    let signals = read_db
        .get_cross_source_overlap_signals()
        .unwrap_or_default();

    if signals.is_empty() {
        return Ok(DirectoryClusterModalData::default());
    }

    let mut clusters = Vec::new();

    for signal in signals {
        let cluster_key = signal.key;
        let data = signal.data;

        let source_a = data.source_a;
        let source_b = data.source_b;
        let source_a_can_stash = data.source_a_can_stash;
        let source_b_can_stash = data.source_b_can_stash;
        let overlap_count = data.overlap_count;

        let source_a_inodes: Vec<i64> = data
            .track_pairs
            .iter()
            .map(|tp| tp.source_a_inode)
            .collect();
        let source_b_inodes: Vec<i64> = data
            .track_pairs
            .iter()
            .map(|tp| tp.source_b_inode)
            .collect();

        let mut directories = Vec::new();

        for (source_path, inodes, can_stash) in [
            (&source_a, &source_a_inodes, source_a_can_stash),
            (&source_b, &source_b_inodes, source_b_can_stash),
        ] {
            let unique_inodes: Vec<i64> = {
                let mut seen = HashSet::new();
                inodes.iter().copied().filter(|i| seen.insert(*i)).collect()
            };

            let mut paths = Vec::new();
            let mut format_counts: HashMap<String, usize> = HashMap::new();

            for &inode in &unique_inodes {
                if let Ok(Some(audio_file)) =
                    read_db.get_audio_file_by_inode(inode, Zone::Corpus)
                {
                    paths.push(audio_file.path().to_string());
                    *format_counts
                        .entry(audio_file.audio.file_type.to_uppercase())
                        .or_insert(0) += 1;
                }
            }

            let format_summary = format_summary_from_counts(&format_counts);

            directories.push(DirectoryGroupEntry {
                path_suffix: source_path.clone(),
                inodes: unique_inodes,
                paths,
                format_summary,
                can_stash_dupes: can_stash,
            });
        }

        if directories.len() < 2 {
            continue;
        }

        clusters.push(DirectoryClusterEntry {
            cluster_key,
            directories,
            overlap_count,
        });
    }

    let file_meta_cache = build_meta_cache(read_db, &clusters);

    Ok(DirectoryClusterModalData {
        clusters,
        file_meta_cache,
    })
}

/// Load release overlap clusters from the database.
pub fn load_release_overlap_data(
    read_db: &ReadOnlyDb<'_>,
) -> Result<mm_meta::views::cluster_deploy::DirectoryClusterModalData> {
    use mm_meta::views::cluster_deploy::{
        DirectoryClusterEntry, DirectoryClusterModalData, DirectoryGroupEntry,
    };

    let signals = read_db.get_release_overlap_signals().unwrap_or_default();

    if signals.is_empty() {
        return Ok(DirectoryClusterModalData::default());
    }

    let mut clusters = Vec::new();

    for signal in signals {
        let cluster_key = signal.key;
        let data = signal.data;

        let mut directories = Vec::new();

        for entry in &data.releases {
            let path_suffix = if entry.release_dir.is_empty() {
                entry.source_dir.clone()
            } else {
                format!("{}/{}", entry.source_dir, entry.release_dir)
            };

            let unique_inodes: Vec<i64> = {
                let mut seen = HashSet::new();
                entry
                    .inodes
                    .iter()
                    .copied()
                    .filter(|i| seen.insert(*i))
                    .collect()
            };

            let mut paths = Vec::new();
            let mut format_counts: HashMap<String, usize> = HashMap::new();

            for &inode in &unique_inodes {
                if let Ok(Some(audio_file)) =
                    read_db.get_audio_file_by_inode(inode, Zone::Corpus)
                {
                    paths.push(audio_file.path().to_string());
                    *format_counts
                        .entry(audio_file.audio.file_type.to_uppercase())
                        .or_insert(0) += 1;
                }
            }

            let format_summary = format_summary_from_counts(&format_counts);

            directories.push(DirectoryGroupEntry {
                path_suffix,
                inodes: unique_inodes,
                paths,
                format_summary,
                can_stash_dupes: entry.can_stash,
            });
        }

        if directories.len() < 2 {
            continue;
        }

        clusters.push(DirectoryClusterEntry {
            cluster_key,
            directories,
            overlap_count: data.file_count,
        });
    }

    let file_meta_cache = build_meta_cache(read_db, &clusters);

    Ok(DirectoryClusterModalData {
        clusters,
        file_meta_cache,
    })
}

/// Build a metadata cache for all unique inodes across clusters.
fn build_meta_cache(
    read_db: &ReadOnlyDb<'_>,
    clusters: &[mm_meta::views::cluster_deploy::DirectoryClusterEntry],
) -> HashMap<i64, mm_meta::views::review_match::FileMetaSummary> {
    let mut cache = HashMap::new();
    for cluster in clusters {
        for dir in &cluster.directories {
            for &inode in &dir.inodes {
                if cache.contains_key(&inode) {
                    continue;
                }
                if let Some(meta) = load_file_meta_summary(read_db, inode) {
                    cache.insert(inode, meta);
                }
            }
        }
    }
    cache
}

// ============================================================================
// Deploy Modal
// ============================================================================

/// Load all deploy signal data from the database.
pub fn load_deploy_data(
    read_db: &ReadOnlyDb<'_>,
    config: Option<&crate::config::Config>,
) -> Result<mm_meta::views::cluster_deploy::DeployModalData> {
    use mm_meta::views::cluster_deploy::{
        DeployModalData,
    };

    let healthy = read_db.get_deployed_healthy_files()?;
    let mut new = read_db.get_deploy_ready_files()?;
    let conflicts = read_db.get_deploy_conflict_groups()?;
    let leftover = read_db.get_library_leftover_files()?;
    let stale = read_db.get_library_stale_files()?;

    if let Some(cfg) = config {
        for file in &mut new {
            if let Some(lib) = cfg
                .resolve_source_config_for_db_path(&file.corpus_path)
                .and_then(|r| r.libraries.into_iter().next())
            {
                file.library_name = lib;
            }
        }
    }

    let sidecars = load_deploy_sidecars(read_db);
    let sidecar_conflicts = read_db.get_sidecar_conflict_groups().unwrap_or_default();

    crate::logging::log_general(format!(
        "[UI] DeployModalData::load: healthy={}, new={}, conflicts={}, leftover={}, stale={}, sidecars={}, sidecar_conflicts={}",
        healthy.len(), new.len(), conflicts.len(), leftover.len(), stale.len(), sidecars.len(), sidecar_conflicts.len(),
    ));

    let mut new_by_dir = aggregate_by_directory(new.iter().map(|f| f.corpus_path.as_str()));
    merge_sidecar_counts(&mut new_by_dir, &sidecars);

    let leftover_by_dir =
        aggregate_by_directory(leftover.iter().map(|f| f.library_path.as_str()));

    let new_destinations: HashSet<String> = new
        .iter()
        .filter(|f| !f.library_name.is_empty())
        .map(|f| format!("{}/{}", f.library_name, f.deploy_path))
        .collect();

    let replaced_count = leftover
        .iter()
        .filter(|f| new_destinations.contains(&f.library_path))
        .count();

    let per_library = compute_per_library(
        &healthy,
        &new,
        &leftover,
        &stale,
        &conflicts,
        &new_destinations,
    );

    Ok(DeployModalData {
        healthy,
        new,
        new_by_dir,
        conflicts,
        leftover,
        leftover_by_dir,
        stale,
        per_library,
        replaced_count,
        sidecars,
        sidecar_conflicts,
    })
}

fn load_deploy_sidecars(
    read_db: &ReadOnlyDb<'_>,
) -> Vec<mm_meta::views::cluster_deploy::SidecarDeployEntry> {
    use mm_meta::views::cluster_deploy::SidecarDeployEntry;

    let signals = match read_db.get_sidecar_deploy_ready_signals() {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };

    signals
        .into_iter()
        .map(|s| {
            let filename = Path::new(&s.path)
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_default();
            let library_album_dir = Path::new(&s.deploy_path)
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();

            SidecarDeployEntry {
                corpus_image_path: s.path,
                library_name: s.library_name,
                library_album_dir,
                filename,
                format: s.data.format,
                width: s.data.width,
                height: s.data.height,
                role: s.data.role,
            }
        })
        .collect()
}

fn aggregate_by_directory<'a>(
    paths: impl Iterator<Item = &'a str>,
) -> Vec<mm_meta::views::cluster_deploy::DirectoryAggregate> {
    use mm_meta::views::cluster_deploy::DirectoryAggregate;

    let mut counts: HashMap<String, usize> = HashMap::new();
    for path in paths {
        let dir = Path::new(path)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "/".to_string());
        *counts.entry(dir).or_insert(0) += 1;
    }

    let mut aggregates: Vec<_> = counts
        .into_iter()
        .map(|(directory, count)| DirectoryAggregate {
            directory,
            count,
            sidecar_count: 0,
        })
        .collect();

    aggregates.sort_by(|a, b| b.count.cmp(&a.count));
    aggregates
}

fn merge_sidecar_counts(
    new_by_dir: &mut Vec<mm_meta::views::cluster_deploy::DirectoryAggregate>,
    sidecars: &[mm_meta::views::cluster_deploy::SidecarDeployEntry],
) {
    use mm_meta::views::cluster_deploy::DirectoryAggregate;

    if sidecars.is_empty() {
        return;
    }

    let mut sidecar_dir_counts: HashMap<String, usize> = HashMap::new();
    for s in sidecars {
        let dir = Path::new(&s.corpus_image_path)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "/".to_string());
        *sidecar_dir_counts.entry(dir).or_insert(0) += 1;
    }

    for entry in new_by_dir.iter_mut() {
        if let Some(sc_count) = sidecar_dir_counts.remove(&entry.directory) {
            entry.sidecar_count = sc_count;
        }
    }

    for (directory, sc_count) in sidecar_dir_counts {
        new_by_dir.push(DirectoryAggregate {
            directory,
            count: 0,
            sidecar_count: sc_count,
        });
    }

    new_by_dir.sort_by(|a, b| (b.count + b.sidecar_count).cmp(&(a.count + a.sidecar_count)));
}

fn compute_per_library(
    healthy: &[crate::meta::views::DeploySignalFile],
    new: &[crate::meta::views::DeploySignalFile],
    leftover: &[crate::meta::views::LeftoverSignalFile],
    stale: &[crate::meta::views::StaleSignalFile],
    _conflicts: &[crate::meta::views::ConflictGroup],
    new_destinations: &HashSet<String>,
) -> Vec<mm_meta::views::cluster_deploy::LibrarySummary> {
    use mm_meta::views::cluster_deploy::LibrarySummary;

    let mut libs: HashMap<String, LibrarySummary> = HashMap::new();

    for f in healthy {
        let entry = libs
            .entry(f.library_name.clone())
            .or_insert_with(|| LibrarySummary {
                library_name: f.library_name.clone(),
                healthy_count: 0,
                new_count: 0,
                leftover_count: 0,
                stale_count: 0,
                replaced_count: 0,
            });
        entry.healthy_count += 1;
    }
    for f in new {
        if !f.library_name.is_empty() {
            let entry = libs
                .entry(f.library_name.clone())
                .or_insert_with(|| LibrarySummary {
                    library_name: f.library_name.clone(),
                    healthy_count: 0,
                    new_count: 0,
                    leftover_count: 0,
                    stale_count: 0,
                    replaced_count: 0,
                });
            entry.new_count += 1;
        }
    }
    for f in leftover {
        let entry = libs
            .entry(f.library_name.clone())
            .or_insert_with(|| LibrarySummary {
                library_name: f.library_name.clone(),
                healthy_count: 0,
                new_count: 0,
                leftover_count: 0,
                stale_count: 0,
                replaced_count: 0,
            });
        entry.leftover_count += 1;
        if new_destinations.contains(&f.library_path) {
            entry.replaced_count += 1;
        }
    }
    for f in stale {
        let entry = libs
            .entry(f.library_name.clone())
            .or_insert_with(|| LibrarySummary {
                library_name: f.library_name.clone(),
                healthy_count: 0,
                new_count: 0,
                leftover_count: 0,
                stale_count: 0,
                replaced_count: 0,
            });
        entry.stale_count += 1;
    }

    if libs.len() < 2 {
        return Vec::new();
    }

    let mut result: Vec<_> = libs.into_values().collect();
    result.sort_by(|a, b| a.library_name.cmp(&b.library_name));
    result
}

// ============================================================================
// Manual Review Modal
// ============================================================================

/// Load audio metadata summary for a single inode from the database.
pub fn load_file_meta_summary(
    read_db: &ReadOnlyDb<'_>,
    inode: i64,
) -> Option<mm_meta::views::review_match::FileMetaSummary> {
    use mm_meta::views::review_match::FileMetaSummary;

    let info = read_db.get_audio_info(inode).ok().flatten()?;
    let tags = read_db
        .get_tags::<crate::zones::CorpusZone>(inode)
        .ok()
        .unwrap_or_default();
    let has_pictures = read_db.get_has_pictures(inode).unwrap_or(false);
    let file_size = read_db
        .get_audio_file_by_inode(inode, Zone::Corpus)
        .ok()
        .flatten()
        .map(|af| af.entry.file_size)
        .unwrap_or(0);

    Some(FileMetaSummary {
        file_type: info.file_type,
        duration_ms: info.duration_ms,
        bitrate_kbps: info.bitrate_kbps,
        sample_rate: info.sample_rate,
        file_size,
        has_pictures,
        tags: tags
            .into_iter()
            .map(|t| (t.tag_name, t.tag_value))
            .collect(),
    })
}

/// Load review data from the database based on review kind.
pub fn load_manual_review_data(
    read_db: &ReadOnlyDb<'_>,
    kind: mm_meta::views::review_match::ReviewKind,
) -> Result<mm_meta::views::review_match::ManualReviewData> {
    use mm_meta::views::review_match::ReviewKind;

    let mut data = match kind {
        ReviewKind::RedundantDuplicate => load_redundant_duplicates(read_db)?,
        ReviewKind::DeployConflict => load_deploy_conflicts(read_db)?,
        ReviewKind::MetadataDuplicate => load_metadata_duplicates(read_db)?,
        ReviewKind::SameRecordingDifferentRelease => load_same_recording_different_releases(read_db)?,
    };

    // Enrich with metadata
    for group in &mut data.groups {
        for file in &mut group.files {
            file.meta = load_file_meta_summary(read_db, file.inode);
        }
    }

    Ok(data)
}

fn load_redundant_duplicates(
    read_db: &ReadOnlyDb<'_>,
) -> Result<mm_meta::views::review_match::ManualReviewData> {
    use mm_meta::views::review_match::{ManualReviewData, ReviewFileEntry, ReviewGroup};

    let signal_groups = read_db.get_redundant_duplicate_groups()?;

    let mut groups = Vec::new();
    for (key, data) in signal_groups {
        let label = format!("{} ({}×)", data.file_type, data.inodes.len());

        let mut files = Vec::new();
        for (idx, &inode) in data.inodes.iter().enumerate() {
            let path = data.paths.get(idx).cloned().unwrap_or_else(|| {
                read_db
                    .get_path_for_inode::<crate::zones::CorpusZone>(inode)
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| format!("<inode {}>", inode))
            });

            files.push(ReviewFileEntry {
                corpus_path: path,
                inode,
                context: "Same fingerprint, equivalent quality".to_string(),
                stashed: false,
                meta: None,
            });
        }

        if files.len() >= 2 {
            groups.push(ReviewGroup {
                label,
                files,
                signal_key: Some(key),
            });
        }
    }

    Ok(ManualReviewData { groups })
}

fn load_deploy_conflicts(
    read_db: &ReadOnlyDb<'_>,
) -> Result<mm_meta::views::review_match::ManualReviewData> {
    use mm_meta::views::review_match::{ManualReviewData, ReviewFileEntry, ReviewGroup};

    let conflict_groups = read_db.get_deploy_conflict_groups()?;

    let mut groups = Vec::new();
    for group in conflict_groups {
        let label = group.deploy_path.clone();

        let files: Vec<ReviewFileEntry> = group
            .conflicting_files
            .into_iter()
            .map(|(corpus_path, inode)| ReviewFileEntry {
                corpus_path,
                inode,
                context: format!("Deploys to: {}", group.deploy_path),
                stashed: false,
                meta: None,
            })
            .collect();

        if files.len() >= 2 {
            groups.push(ReviewGroup {
                label,
                files,
                signal_key: None,
            });
        }
    }

    Ok(ManualReviewData { groups })
}

fn load_metadata_duplicates(
    read_db: &ReadOnlyDb<'_>,
) -> Result<mm_meta::views::review_match::ManualReviewData> {
    use mm_meta::views::review_match::{ManualReviewData, ReviewFileEntry, ReviewGroup};

    let signal_groups = read_db.get_metadata_duplicate_groups()?;

    let mut groups = Vec::new();
    for (_key, data) in signal_groups {
        let label = data.tag_signature.clone();

        let mut files = Vec::new();
        for &inode in &data.inodes {
            let path = read_db
                .get_path_for_inode::<crate::zones::CorpusZone>(inode)
                .ok()
                .flatten()
                .unwrap_or_else(|| format!("<inode {}>", inode));

            files.push(ReviewFileEntry {
                corpus_path: path,
                inode,
                context: data.tag_signature.clone(),
                stashed: false,
                meta: None,
            });
        }

        if files.len() >= 2 {
            groups.push(ReviewGroup {
                label,
                files,
                signal_key: None,
            });
        }
    }

    Ok(ManualReviewData { groups })
}

fn load_same_recording_different_releases(
    read_db: &ReadOnlyDb<'_>,
) -> Result<mm_meta::views::review_match::ManualReviewData> {
    use mm_meta::views::review_match::{ManualReviewData, ReviewFileEntry, ReviewGroup};

    let signal_groups = read_db.get_same_recording_different_release_groups()?;

    let mut groups = Vec::new();
    for (key, data) in signal_groups {
        let label = format!("Recording {}", key);

        let mut files = Vec::new();
        for entry in &data.entries {
            files.push(ReviewFileEntry {
                corpus_path: entry.path.clone(),
                inode: entry.inode,
                context: format!("{} (release: {})", entry.album, entry.mb_release_id),
                stashed: false,
                meta: None,
            });
        }

        if files.len() >= 2 {
            groups.push(ReviewGroup {
                label,
                files,
                signal_key: Some(key),
            });
        }
    }

    Ok(ManualReviewData { groups })
}

// ============================================================================
// Inbox Corpus Match Modal
// ============================================================================

/// Load inbox corpus match entries from the database.
pub fn load_inbox_corpus_match_data(
    read_db: &ReadOnlyDb<'_>,
    bitrate_fuzz_percent: f64,
) -> Result<mm_meta::views::review_match::InboxCorpusMatchModalData> {
    use crate::meta::views::MatchClassification;
    use mm_meta::views::review_match::InboxCorpusMatchModalData;

    let mut entries = read_db.get_inbox_corpus_match_entries(bitrate_fuzz_percent)?;

    entries.sort_by_key(|e| match e.classification {
        MatchClassification::Equivalent => 0,
        MatchClassification::Subpar => 1,
        MatchClassification::Better => 2,
    });

    Ok(InboxCorpusMatchModalData { entries })
}

// ============================================================================
// Compound Split V2
// ============================================================================

/// Load compound split modal data for a specific compound group.
#[deprecated(note = "use load_compound_split_resolution")]
pub fn load_compound_split_data(
    group: &crate::meta::signals::data::CompoundGroup,
    read_db: &ReadOnlyDb,
    zone: Zone,
) -> Option<mm_meta::views::canonicity_compound::CompoundSplitDataV2> {
    use mm_meta::views::canonicity_compound::{CompoundEntry, CompoundSplitDataV2, FileTagInfo};

    if group.inodes.is_empty() {
        return None;
    }

    let first_compounds = if zone == Zone::Inbox {
        read_db
            .get_inbox_compound_tag_signal(group.inodes[0])
            .ok()??
            .compounds
    } else {
        read_db
            .get_compound_tag_signal(group.inodes[0])
            .ok()??
            .compounds
    };
    let c = first_compounds
        .iter()
        .find(|c| c.tag_name == group.tag_name && c.compound_value == group.compound_value)?;
    let compound = CompoundEntry {
        tag_name: c.tag_name.clone(),
        compound_value: c.compound_value.clone(),
        split_parts: c.split_parts.clone(),
        matching_parts: c.matching_parts.clone(),
    };

    let resolver = paths::get_resolver();
    let mut files = Vec::new();

    for &inode in &group.inodes {
        if let Ok(Some(audio_file)) = read_db.get_audio_file_by_inode(inode, zone) {
            let path = audio_file.path();
            let filename = Path::new(path)
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| path.to_string());

            let abs_path = resolver.resolve(Path::new(path));
            let tagset = tags::from_file(&abs_path).unwrap_or_else(|_| TagSet::empty());

            let tag_values: Vec<(String, String)> = tagset
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();

            files.push(FileTagInfo {
                inode,
                filename,
                path: path.to_string(),
                tag_values,
            });
        }
    }

    files.sort_by(|a, b| a.filename.cmp(&b.filename));

    Some(CompoundSplitDataV2 { compound, files })
}

// ============================================================================
// Tag Canonicity V2
// ============================================================================

/// Load file info (filename, path, tags) for a set of inodes from a specific zone.
// DEPRECATED: reads from disk; use get_display_names_batch + get_tag_values_batch
fn load_file_info_for_zone(
    inodes: &[i64],
    read_db: &ReadOnlyDb,
    zone: Zone,
) -> Vec<mm_meta::views::canonicity_compound::FileTagInfo> {
    use mm_meta::views::canonicity_compound::FileTagInfo;

    let resolver = paths::get_resolver();
    let mut files = Vec::new();

    for &inode in inodes {
        if let Ok(Some(audio_file)) = read_db.get_audio_file_by_inode(inode, zone) {
            let path = audio_file.path();
            let filename = Path::new(path)
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| path.to_string());

            let abs_path = resolver.resolve(Path::new(path));
            let tagset = tags::from_file(&abs_path).unwrap_or_else(|_| TagSet::empty());

            let tag_values: Vec<(String, String)> = tagset
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();

            files.push(FileTagInfo {
                inode,
                filename,
                path: path.to_string(),
                tag_values,
            });
        }
    }

    files.sort_by(|a, b| a.filename.cmp(&b.filename));
    files
}

/// Create TagCanonicalityModalDataV2 from a typed `TagCanonicitySignal`.
#[deprecated(note = "use load_tag_canonicity_resolution")]
pub fn load_tag_canonicity_data(
    signal: &crate::meta::signals::data::TagCanonicitySignal,
    read_db: &ReadOnlyDb,
) -> Option<mm_meta::views::canonicity_compound::TagCanonicalityModalDataV2> {
    use mm_meta::views::canonicity_compound::{TagCanonicalityModalDataV2, TagVariantEntry};

    let tag_name = signal.tag_name.clone();

    let mut variants: Vec<TagVariantEntry> = signal
        .data
        .variants
        .iter()
        .map(|(value, count)| TagVariantEntry {
            value: value.clone(),
            count: *count,
        })
        .collect();

    variants.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.value.cmp(&b.value)));

    let inodes = signal.data.inodes.clone();
    let files = load_file_info_for_zone(&inodes, read_db, Zone::Corpus);

    Some(TagCanonicalityModalDataV2 {
        tag_name,
        context_label: None,
        variants,
        inodes,
        files,
        default_canonical_override: None,
    })
}

/// Create TagCanonicalityModalDataV2 from a typed `InconsistentAlbumArtistSignal`.
pub fn load_inconsistent_album_artist_data(
    signal: &crate::meta::signals::data::InconsistentAlbumArtistSignal,
    read_db: &ReadOnlyDb,
) -> Option<mm_meta::views::canonicity_compound::TagCanonicalityModalDataV2> {
    use mm_meta::views::canonicity_compound::{TagCanonicalityModalDataV2, TagVariantEntry};

    let tag_name = "ALBUMARTIST".to_string();
    let context_label = Some(signal.data.album.clone());

    let mut variants: Vec<TagVariantEntry> = signal
        .data
        .album_artist_variants
        .iter()
        .map(|(value, count)| TagVariantEntry {
            value: value.clone(),
            count: *count,
        })
        .collect();

    variants.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.value.cmp(&b.value)));

    let inodes = signal.data.inodes.clone();
    let files = load_file_info_for_zone(&inodes, read_db, Zone::Corpus);

    Some(TagCanonicalityModalDataV2 {
        tag_name,
        context_label,
        variants,
        inodes,
        files,
        default_canonical_override: None,
    })
}

/// Create TagCanonicalityModalDataV2 from an `InboxTagCanonicitySignal`.
pub fn load_inbox_tag_canonicity_data(
    signal: &crate::meta::signals::data::InboxTagCanonicitySignal,
    read_db: &ReadOnlyDb,
) -> Option<mm_meta::views::canonicity_compound::TagCanonicalityModalDataV2> {
    use mm_meta::views::canonicity_compound::{TagCanonicalityModalDataV2, TagVariantEntry};

    let tag_name = signal.tag_name.clone();

    let mut variants: Vec<TagVariantEntry> = signal
        .data
        .inbox_variants
        .iter()
        .map(|(value, count)| TagVariantEntry {
            value: value.clone(),
            count: *count,
        })
        .collect();

    variants.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.value.cmp(&b.value)));

    let corpus_display: Vec<String> = signal
        .data
        .corpus_variants
        .iter()
        .map(|(v, c)| format!("{} ({})", v, c))
        .collect();
    let context_label = Some(format!("Corpus: {}", corpus_display.join(", ")));

    let inodes = signal.data.inbox_inodes.clone();
    let files = load_file_info_for_zone(&inodes, read_db, Zone::Inbox);

    let default_canonical_override =
        signal.data.corpus_variants.first().map(|(v, _)| v.clone());

    Some(TagCanonicalityModalDataV2 {
        tag_name,
        context_label,
        variants,
        inodes,
        files,
        default_canonical_override,
    })
}

// ============================================================================
// Packed Tag Canonicity Resolution (single-load, all clusters)
// ============================================================================

/// Load all canonicity clusters for a tag+zone in one response.
///
/// Replaces the two-phase GetTagCanonicityKeys + GetTagCanonicitySignalData
/// pattern with a single query that returns all clusters at once, using
/// DB-cached tags instead of disk reads.
pub fn load_tag_canonicity_resolution(
    tag_name: &str,
    zone: Zone,
    filter_existing_canonicals: bool,
    db: &ReadOnlyDb,
) -> mm_meta::views::canonicity_compound::TagCanonicityResolutionData {
    use mm_meta::views::canonicity_compound::{
        CanonicityCluster, Variant, ResolutionFileInfo, TagCanonicityResolutionData,
    };

    let tag_prefix = format!("{}:", tag_name);

    // Step 1: Get all signal keys for this tag name, then load each signal
    let all_keys = match zone {
        Zone::Inbox => db
            .aggregate_signal_keys::<crate::meta::signals::data::InboxTagCanonicitySignal>()
            .unwrap_or_default(),
        _ => db
            .aggregate_signal_keys::<crate::meta::signals::data::TagCanonicitySignal>()
            .unwrap_or_default(),
    };

    let filtered_keys: Vec<&String> = all_keys
        .iter()
        .filter(|k| k.starts_with(&tag_prefix))
        .collect();

    if filtered_keys.is_empty() {
        return TagCanonicityResolutionData {
            tag_name: tag_name.to_string(),
            clusters: Vec::new(),
        };
    }

    // Step 2: Check for existing CanonicalTagSignal emissions
    let canonical_signals: HashSet<String> = db
        .aggregate_signal_keys::<crate::meta::signals::data::CanonicalTagSignal>()
        .unwrap_or_default()
        .into_iter()
        .collect();

    // Step 3: Load each signal and collect clusters with ALL variants
    let mut clusters: Vec<CanonicityCluster> = Vec::new();
    let mut all_inodes: Vec<i64> = Vec::new();

    for key in &filtered_keys {
        // Check if this cluster is already covered by a canonical signal
        if filter_existing_canonicals && canonical_signals.contains(key.as_str()) {
            continue;
        }
        let confirmed = if canonical_signals.contains(key.as_str()) {
            // A canonical signal exists — extract the confirmed value from the signal key
            // Signal keys are formatted as "TAG_NAME:value", extract the value part
            key.strip_prefix(&tag_prefix).map(|v| v.to_string())
        } else {
            None
        };

        match zone {
            Zone::Inbox => {
                let Some(signal) = db.get_inbox_tag_canonicity_signal(key).ok().flatten() else {
                    continue;
                };
                // Suggested canonical = top corpus variant
                let suggested = signal.data.corpus_variants
                    .first()
                    .map(|(v, _)| v.clone());

                // ALL variants: corpus + inbox
                let all_variant_inodes = signal.data.inbox_inodes.clone();
                all_inodes.extend(&all_variant_inodes);

                let variants: Vec<Variant> = signal.data.inbox_variants
                    .iter()
                    .map(|(value, _)| Variant {
                        value: value.clone(),
                        files: all_variant_inodes.iter()
                            .map(|&inode| ResolutionFileInfo { inode, display_name: String::new() })
                            .collect(),
                    })
                    .collect();

                clusters.push(CanonicityCluster {
                    signal_key: key.to_string(),
                    confirmed_canonical: confirmed.clone(),
                    suggested_canonical: confirmed.or(suggested),
                    variants,
                });
            }
            _ => {
                let Some(signal) = db.get_tag_canonicity_signal(key).ok().flatten() else {
                    continue;
                };
                // Suggested canonical = most common variant (first in sorted-by-count list)
                let suggested = signal.data.variants
                    .first()
                    .map(|(v, _)| v.clone());

                // Query actual tag values per inode to build accurate variant→file mapping
                let tag_values_map = db
                    .get_tag_values_batch(zone, tag_name, &signal.data.inodes)
                    .unwrap_or_default();

                // Group ALL inodes by their actual tag value
                let mut variant_inodes: HashMap<String, Vec<i64>> = HashMap::new();
                for &inode in &signal.data.inodes {
                    if let Some(values) = tag_values_map.get(&inode) {
                        for value in values {
                            variant_inodes
                                .entry(value.clone())
                                .or_default()
                                .push(inode);
                        }
                    }
                }
                all_inodes.extend(&signal.data.inodes);

                // If tag query returned nothing, fall back to signal variant list
                if variant_inodes.is_empty() {
                    for (value, _) in &signal.data.variants {
                        for &inode in &signal.data.inodes {
                            variant_inodes.entry(value.clone()).or_default().push(inode);
                        }
                    }
                }

                // Build ALL variants (no skip, no filtering)
                let variants: Vec<Variant> = signal.data.variants
                    .iter()
                    .filter_map(|(value, _)| {
                        let inodes = variant_inodes.get(value)?;
                        Some(Variant {
                            value: value.clone(),
                            files: inodes.iter()
                                .map(|&inode| ResolutionFileInfo { inode, display_name: String::new() })
                                .collect(),
                        })
                    })
                    .collect();

                clusters.push(CanonicityCluster {
                    signal_key: key.to_string(),
                    confirmed_canonical: confirmed.clone(),
                    suggested_canonical: confirmed.or(suggested),
                    variants,
                });
            }
        }
    }

    // Step 4: Batch-query display names for all inodes
    all_inodes.sort_unstable();
    all_inodes.dedup();
    let display_names = db
        .get_display_names_batch(zone, &all_inodes)
        .unwrap_or_default();

    // Step 5: Fill in display names
    for cluster in &mut clusters {
        for variant in &mut cluster.variants {
            for file in &mut variant.files {
                if let Some(name) = display_names.get(&file.inode) {
                    file.display_name = name.clone();
                } else {
                    file.display_name = format!("<inode {}>", file.inode);
                }
            }
        }
    }

    TagCanonicityResolutionData {
        tag_name: tag_name.to_string(),
        clusters,
    }
}

// ============================================================================
// Packed Compound Split Resolution (single-load, all groups)
// ============================================================================

/// Load all compound split groups for a tag+zone in one response.
///
/// Replaces the two-phase GetCompoundSignalGroups + GetCompoundSplitGroupData
/// pattern with a single query that returns all groups at once, using
/// DB-cached tags instead of disk reads.
pub fn load_compound_split_resolution(
    tag_name: &str,
    zone: Zone,
    safe_only: bool,
    db: &ReadOnlyDb,
) -> mm_meta::views::canonicity_compound::CompoundSplitResolutionData {
    use mm_meta::views::canonicity_compound::{
        CompoundSplitCluster, CompoundSplitResolutionData, ResolutionFileInfo,
    };

    // Step 1: Get compound groups filtered by tag name and safety
    let groups = match zone {
        Zone::Inbox => {
            let all_groups = db.get_inbox_compound_signal_groups().unwrap_or_default();
            all_groups
                .into_iter()
                .filter(|g| g.tag_name == tag_name)
                .collect::<Vec<_>>()
        }
        _ => db
            .get_compound_signal_groups_by_safety(safe_only, Some(tag_name))
            .unwrap_or_default(),
    };

    if groups.is_empty() {
        return CompoundSplitResolutionData {
            groups: Vec::new(),
        };
    }

    // Step 2: Collect all inodes across all groups for batch display name lookup
    let mut all_inodes: Vec<i64> = groups.iter().flat_map(|g| g.inodes.iter().copied()).collect();
    all_inodes.sort_unstable();
    all_inodes.dedup();

    // Step 3: Batch-query display names
    let display_names = db
        .get_display_names_batch(zone, &all_inodes)
        .unwrap_or_default();

    // Step 4: For each group, get the compound entry data (split_parts, matching_parts)
    // by reading the signal from the first inode in the group
    let mut result_groups: Vec<CompoundSplitCluster> = Vec::with_capacity(groups.len());
    for group in &groups {
        // Get split_parts and matching_parts from the compound tag signal
        let (split_parts, matching_parts) = if let Some(first_inode) = group.inodes.first() {
            let compounds = match zone {
                Zone::Inbox => db
                    .get_inbox_compound_tag_signal(*first_inode)
                    .ok()
                    .flatten()
                    .map(|s| s.compounds),
                _ => db
                    .get_compound_tag_signal(*first_inode)
                    .ok()
                    .flatten()
                    .map(|s| s.compounds),
            };
            compounds
                .and_then(|cs| {
                    cs.iter()
                        .find(|c| {
                            c.tag_name == group.tag_name
                                && c.compound_value == group.compound_value
                        })
                        .map(|c| (c.split_parts.clone(), c.matching_parts.clone()))
                })
                .unwrap_or_default()
        } else {
            (Vec::new(), Vec::new())
        };

        let files: Vec<ResolutionFileInfo> = group
            .inodes
            .iter()
            .map(|&inode| ResolutionFileInfo {
                inode,
                display_name: display_names
                    .get(&inode)
                    .cloned()
                    .unwrap_or_else(|| format!("<inode {}>", inode)),
            })
            .collect();

        result_groups.push(CompoundSplitCluster {
            tag_name: group.tag_name.clone(),
            compound_value: group.compound_value.clone(),
            split_parts,
            matching_parts,
            files,
        });
    }

    CompoundSplitResolutionData {
        groups: result_groups,
    }
}

// ============================================================================
// Intake Confirmation
// ============================================================================

/// Gather intake confirmation state for a specific zone.
pub fn gather_intake_zone<Z: crate::zones::AudioZone>(
    read_db: &ReadOnlyDb<'_>,
    source: mm_meta::views::startup_organize::IntakeSource,
) -> Option<mm_meta::views::startup_organize::IntakeConfirmationState> {
    use crate::logging::log_general;
    use mm_meta::views::startup_organize::{
        DirectoryGroup, IntakeConfirmationState, UnindexedFileEntry,
    };

    let unindexed = match read_db.get_unindexed_signals_for::<Z>() {
        Ok(u) => u,
        Err(e) => {
            crate::logging::log_error(format!(
                "IntakeConfirmation::gather_zone<{}>: query failed: {:?}",
                Z::ZONE_STR, e
            ));
            return None;
        }
    };

    log_general(format!(
        "IntakeConfirmation::gather_zone<{}>: found {} unindexed signals",
        Z::ZONE_STR,
        unindexed.len()
    ));

    if unindexed.is_empty() {
        return None;
    }

    let resolver = paths::get_resolver();
    let mut files: Vec<UnindexedFileEntry> = Vec::new();
    let mut total_bytes: u64 = 0;
    let mut directories: HashSet<PathBuf> = HashSet::new();
    let mut dir_to_files: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for (_inode, rel_path_str) in &unindexed {
        let rel_path = std::path::Path::new(rel_path_str);
        let abs_path = resolver.resolve(rel_path);

        if abs_path.exists() && abs_path.is_file() {
            if let Ok(meta) = std::fs::metadata(&abs_path) {
                total_bytes += meta.len();
            }

            if let Some(parent) = abs_path.parent() {
                directories.insert(parent.to_path_buf());
            }

            if let (Some(parent), Some(filename)) = (rel_path.parent(), rel_path.file_name()) {
                let dir_str = parent.to_string_lossy().to_string();
                let file_str = filename.to_string_lossy().to_string();
                dir_to_files.entry(dir_str).or_default().push(file_str);
            }

            files.push(UnindexedFileEntry {
                abs_path,
                zone: Z::ZONE,
            });
        }
    }

    if files.is_empty() {
        return None;
    }

    let grouped_files: Vec<DirectoryGroup> = dir_to_files
        .into_iter()
        .map(|(dir, mut filenames)| {
            filenames.sort();
            DirectoryGroup {
                display_path: dir,
                filenames,
                zone: Z::ZONE,
            }
        })
        .collect();

    log_general(format!(
        "IntakeConfirmation ({}): gathered {} files ({} bytes) from {} directories",
        Z::ZONE_STR,
        files.len(),
        total_bytes,
        directories.len()
    ));

    Some(IntakeConfirmationState {
        file_count: files.len(),
        total_bytes,
        files,
        source,
        multi_zone: false,
        _directory_count: directories.len(),
        grouped_files,
        scroll_offset: 0,
    })
}

/// Gather intake confirmation state for startup: checks both corpus AND inbox.
pub fn gather_intake_startup(
    read_db: &ReadOnlyDb<'_>,
) -> Option<mm_meta::views::startup_organize::IntakeConfirmationState> {
    use crate::logging::log_general;
    use mm_meta::views::startup_organize::IntakeConfirmationState;
    use mm_meta::views::startup_organize::IntakeSource;
    use crate::zones::{CorpusZone, InboxZone};

    let corpus_state = gather_intake_zone::<CorpusZone>(read_db, IntakeSource::Startup);
    let inbox_state = gather_intake_zone::<InboxZone>(read_db, IntakeSource::Startup);

    match (corpus_state, inbox_state) {
        (None, None) => None,
        (Some(mut state), None) => {
            state.source = IntakeSource::Startup;
            Some(state)
        }
        (None, Some(mut state)) => {
            state.source = IntakeSource::Startup;
            Some(state)
        }
        (Some(corpus), Some(inbox)) => {
            let mut files = corpus.files;
            files.extend(inbox.files);

            let mut grouped_files = corpus.grouped_files;
            grouped_files.extend(inbox.grouped_files);

            let file_count = files.len();
            let total_bytes = corpus.total_bytes + inbox.total_bytes;
            let directory_count = corpus._directory_count + inbox._directory_count;

            log_general(format!(
                "IntakeConfirmation (startup): merged {} corpus + {} inbox = {} total files",
                corpus.file_count, inbox.file_count, file_count
            ));

            Some(IntakeConfirmationState {
                file_count,
                total_bytes,
                files,
                source: IntakeSource::Startup,
                multi_zone: true,
                _directory_count: directory_count,
                grouped_files,
                scroll_offset: 0,
            })
        }
    }
}
