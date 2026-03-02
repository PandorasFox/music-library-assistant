//! Deploy Modal Types
//!
//! Data structures for the deploy modal, including cached signal data
//! and per-library breakdown.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::config::SidecarDeployMode;
use crate::corpus::deploy::deploy_album_directory;
use crate::meta::views::{ConflictGroup, DeploySignalFile, LeftoverSignalFile, StaleSignalFile};
use crate::db::ReadOnlyDb;
use anyhow::Result;

/// Directory aggregate for grouped file display.
/// Sorted by count descending.
#[derive(Debug, Clone)]
pub struct DirectoryAggregate {
    /// Directory path
    pub directory: String,
    /// Number of files in this directory
    pub count: usize,
}

/// Per-library breakdown of deploy operations.
#[derive(Debug, Clone)]
pub struct LibrarySummary {
    pub library_name: String,
    pub healthy_count: usize,
    pub new_count: usize,
    pub leftover_count: usize,
    pub stale_count: usize,
    /// Leftovers whose library_path matches a pending new deployment destination
    pub replaced_count: usize,
}

/// A sidecar image file to deploy alongside audio files.
#[derive(Debug, Clone)]
pub struct SidecarDeployEntry {
    /// Relative corpus path to the image file
    pub corpus_image_path: String,
    /// Target library name
    pub library_name: String,
    /// Album directory within the library (e.g. "Artist/Album")
    pub library_album_dir: String,
    /// Original filename (e.g. "cover.jpg")
    pub filename: String,
    /// Image format (e.g. "jpeg", "png")
    pub format: String,
    /// Image width in pixels
    pub width: u32,
    /// Image height in pixels
    pub height: u32,
    /// Image role (e.g. "cover_front", "cover_back", "other")
    pub role: String,
}

/// Cached data for the deploy modal.
///
/// Loaded once when the modal opens, contains all signal lists.
/// This prevents database queries during render.
#[derive(Debug, Clone, Default)]
pub struct DeployModalData {
    /// Healthy files: deployed at correct library path
    pub healthy: Vec<DeploySignalFile>,
    /// New files: ready to deploy (not yet in library)
    pub new: Vec<DeploySignalFile>,
    /// New files aggregated by directory (sorted by count desc)
    pub new_by_dir: Vec<DirectoryAggregate>,
    /// Conflict groups: multiple corpus files → same library path
    pub conflicts: Vec<ConflictGroup>,
    /// Leftover files: in library but no corpus backing
    pub leftover: Vec<LeftoverSignalFile>,
    /// Leftover files aggregated by directory (sorted by count desc)
    pub leftover_by_dir: Vec<DirectoryAggregate>,
    /// Stale files: deployed at wrong path (tags changed)
    pub stale: Vec<StaleSignalFile>,
    /// Per-library breakdown (only populated when > 1 library)
    pub per_library: Vec<LibrarySummary>,
    /// Total leftovers that will be replaced by new deployments
    pub replaced_count: usize,
    /// Sidecar images to deploy alongside audio files
    pub sidecars: Vec<SidecarDeployEntry>,
}

impl DeployModalData {
    /// Load all deploy signal data from the database.
    ///
    /// Called once when the modal opens. All subsequent renders
    /// use this cached data.
    ///
    /// `config` is used to assign library_name to deploy-ready files
    /// (which don't carry library info in the signal itself).
    pub fn load(read_db: &ReadOnlyDb<'_>, config: Option<&crate::config::Config>) -> Result<Self> {
        let healthy = read_db.get_deployed_healthy_files()?;
        let mut new = read_db.get_deploy_ready_files()?;
        let conflicts = read_db.get_deploy_conflict_groups()?;
        let leftover = read_db.get_library_leftover_files()?;
        let stale = read_db.get_library_stale_files()?;

        // Assign library_name to deploy-ready files via config lookup
        if let Some(cfg) = config {
            for file in &mut new {
                if let Some(lib) = cfg.resolve_source_config_for_db_path(&file.corpus_path)
                    .and_then(|r| r.libraries.into_iter().next()) {
                    file.library_name = lib;
                }
            }
        }

        // Compute sidecar images to deploy alongside new audio files
        let sidecars = Self::compute_sidecars(&new, read_db, config);

        crate::logging::log_general(format!(
            "[UI] DeployModalData::load: healthy={}, new={}, conflicts={}, leftover={}, stale={}, sidecars={}",
            healthy.len(), new.len(), conflicts.len(), leftover.len(), stale.len(), sidecars.len(),
        ));

        // Aggregate new files by directory (using corpus_path)
        let new_by_dir = Self::aggregate_by_directory(
            new.iter().map(|f| f.corpus_path.as_str())
        );

        // Aggregate leftover files by directory (using library_path)
        let leftover_by_dir = Self::aggregate_by_directory(
            leftover.iter().map(|f| f.library_path.as_str())
        );

        // Compute "replaced" leftovers: leftovers whose library_path matches a new deployment
        // New files deploy to "{library_name}/{deploy_path}", which matches leftover library_path format
        let new_destinations: HashSet<String> = new.iter()
            .filter(|f| !f.library_name.is_empty())
            .map(|f| format!("{}/{}", f.library_name, f.deploy_path))
            .collect();

        let replaced_count = leftover.iter()
            .filter(|f| new_destinations.contains(&f.library_path))
            .count();

        // Build per-library breakdown
        let per_library = Self::compute_per_library(
            &healthy, &new, &leftover, &stale, &conflicts, &new_destinations,
        );

        Ok(Self {
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
        })
    }

    /// Compute per-library summaries. Only populated when 2+ libraries are present.
    fn compute_per_library(
        healthy: &[DeploySignalFile],
        new: &[DeploySignalFile],
        leftover: &[LeftoverSignalFile],
        stale: &[StaleSignalFile],
        _conflicts: &[ConflictGroup],
        new_destinations: &HashSet<String>,
    ) -> Vec<LibrarySummary> {
        let mut libs: HashMap<String, LibrarySummary> = HashMap::new();

        for f in healthy {
            let entry = libs.entry(f.library_name.clone()).or_insert_with(|| LibrarySummary {
                library_name: f.library_name.clone(), healthy_count: 0, new_count: 0,
                leftover_count: 0, stale_count: 0, replaced_count: 0,
            });
            entry.healthy_count += 1;
        }
        for f in new {
            if !f.library_name.is_empty() {
                let entry = libs.entry(f.library_name.clone()).or_insert_with(|| LibrarySummary {
                    library_name: f.library_name.clone(), healthy_count: 0, new_count: 0,
                    leftover_count: 0, stale_count: 0, replaced_count: 0,
                });
                entry.new_count += 1;
            }
        }
        for f in leftover {
            let entry = libs.entry(f.library_name.clone()).or_insert_with(|| LibrarySummary {
                library_name: f.library_name.clone(), healthy_count: 0, new_count: 0,
                leftover_count: 0, stale_count: 0, replaced_count: 0,
            });
            entry.leftover_count += 1;
            if new_destinations.contains(&f.library_path) {
                entry.replaced_count += 1;
            }
        }
        for f in stale {
            let entry = libs.entry(f.library_name.clone()).or_insert_with(|| LibrarySummary {
                library_name: f.library_name.clone(), healthy_count: 0, new_count: 0,
                leftover_count: 0, stale_count: 0, replaced_count: 0,
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

    /// Compute sidecar images to deploy alongside new audio files.
    ///
    /// Groups new files by corpus directory, queries image_info for each,
    /// applies SidecarDeployMode filter, and deduplicates by target path.
    fn compute_sidecars(
        new_files: &[DeploySignalFile],
        read_db: &ReadOnlyDb<'_>,
        config: Option<&crate::config::Config>,
    ) -> Vec<SidecarDeployEntry> {
        let cfg = match config {
            Some(c) => c,
            None => return Vec::new(),
        };

        let mode = cfg.opinions.album_art.sidecar_deploy_mode;
        if mode == SidecarDeployMode::Disabled {
            return Vec::new();
        }

        // Group new files by (corpus_parent_dir, library_name, library_album_dir)
        // We need the library album dir from the deploy path
        let mut dir_targets: HashMap<String, (String, String)> = HashMap::new();
        for file in new_files {
            if file.library_name.is_empty() || file.deploy_path.is_empty() {
                continue;
            }
            let corpus_dir = Path::new(&file.corpus_path)
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            let album_dir = deploy_album_directory(&file.deploy_path);
            dir_targets.entry(corpus_dir)
                .or_insert_with(|| (file.library_name.clone(), album_dir));
        }

        // For each corpus directory, query image files and build sidecar entries
        let mut seen: HashSet<(String, String, String)> = HashSet::new(); // (library, album_dir, filename)
        let mut sidecars = Vec::new();

        for (corpus_dir, (library_name, album_dir)) in &dir_targets {
            let images = match read_db.get_corpus_images_in_directory(corpus_dir) {
                Ok(imgs) => imgs,
                Err(_) => continue,
            };

            for img in &images {
                // Apply mode filter
                match mode {
                    SidecarDeployMode::PrimaryCover => {
                        if img.role != "cover_front" {
                            continue;
                        }
                    }
                    SidecarDeployMode::All => {} // accept all
                    SidecarDeployMode::Disabled => unreachable!(),
                }

                let filename = Path::new(&img.path)
                    .file_name()
                    .map(|f| f.to_string_lossy().to_string())
                    .unwrap_or_default();

                if filename.is_empty() {
                    continue;
                }

                // Deduplicate by (library, album_dir, filename)
                let key = (library_name.clone(), album_dir.clone(), filename.clone());
                if !seen.insert(key) {
                    continue;
                }

                sidecars.push(SidecarDeployEntry {
                    corpus_image_path: img.path.clone(),
                    library_name: library_name.clone(),
                    library_album_dir: album_dir.clone(),
                    filename,
                    format: img.format.clone(),
                    width: img.width,
                    height: img.height,
                    role: img.role.clone(),
                });
            }
        }

        sidecars
    }

    /// Aggregate paths by their parent directory, sorted by count descending.
    fn aggregate_by_directory<'a>(paths: impl Iterator<Item = &'a str>) -> Vec<DirectoryAggregate> {
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
            .map(|(directory, count)| DirectoryAggregate { directory, count })
            .collect();

        // Sort by count descending
        aggregates.sort_by(|a, b| b.count.cmp(&a.count));
        aggregates
    }

    /// Get counts for each tab (for display in tab bar).
    pub fn tab_counts(&self) -> [usize; 5] {
        [
            self.healthy.len(),
            self.new.len(),
            self.conflicts.len(),
            self.leftover.len(),
            self.stale.len(),
        ]
    }

    /// Total operations that will be performed (excluding healthy).
    /// Conflicts are auto-resolved by picking first alphabetical path.
    pub fn total_operations(&self) -> usize {
        self.new.len() + self.stale.len() + self.leftover.len() + self.conflicts.len()
    }
}
