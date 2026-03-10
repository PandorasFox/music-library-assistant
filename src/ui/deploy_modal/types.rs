//! Deploy Modal Types
//!
//! Data structures for the deploy modal, including cached signal data
//! and per-library breakdown.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::db::ReadOnlyDb;
use crate::meta::views::{
    ConflictGroup, DeploySignalFile, LeftoverSignalFile, SidecarConflictGroup, StaleSignalFile,
};
use anyhow::Result;

/// Directory aggregate for grouped file display.
/// Sorted by count descending.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DirectoryAggregate {
    /// Directory path
    pub directory: String,
    /// Number of audio files in this directory
    pub count: usize,
    /// Number of sidecar images in this directory
    pub sidecar_count: usize,
}

/// Per-library breakdown of deploy operations.
#[derive(Debug, Clone, serde::Serialize)]
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
#[derive(Debug, Clone, serde::Serialize)]
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

impl SidecarDeployEntry {
    /// Produce a HardLink mutation to deploy this sidecar image.
    pub fn to_mutation(
        &self,
        resolver: &crate::corpus::paths::PathResolver,
    ) -> crate::meta::mutations::Mutation {
        let source = resolver.resolve(std::path::Path::new(&self.corpus_image_path));
        let dest_rel = std::path::Path::new("libraries")
            .join(&self.library_name)
            .join(&self.library_album_dir)
            .join(&self.filename);
        let destination = resolver.resolve(&dest_rel);
        crate::meta::mutations::Mutation::HardLink(
            crate::meta::mutations::file_ops::HardLinkMutation {
                source,
                destination,
            },
        )
    }
}

/// Result of converting deploy modal data into staged mutations.
pub struct DeployMutationSet {
    /// Mutations for DecisionKey::Deploy (leftovers → stale → new, in execution order)
    pub deploy: Vec<crate::meta::mutations::Mutation>,
    /// Mutations for DecisionKey::DeploySidecars
    pub sidecars: Vec<crate::meta::mutations::Mutation>,
    /// Files skipped due to missing library_name (config gap)
    pub skipped: usize,
}

/// Cached data for the deploy modal.
///
/// Loaded once when the modal opens, contains all signal lists.
/// This prevents database queries during render.
#[derive(Debug, Clone, Default, serde::Serialize)]
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
    /// Sidecar image conflicts (multiple corpus images → same library path)
    pub sidecar_conflicts: Vec<SidecarConflictGroup>,
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
                if let Some(lib) = cfg
                    .resolve_source_config_for_db_path(&file.corpus_path)
                    .and_then(|r| r.libraries.into_iter().next())
                {
                    file.library_name = lib;
                }
            }
        }

        // Read precomputed sidecar deploy signals (computed by DeriveCorpusDeployStatus)
        let sidecars = Self::load_sidecars(read_db);
        let sidecar_conflicts = read_db.get_sidecar_conflict_groups().unwrap_or_default();

        crate::logging::log_general(format!(
            "[UI] DeployModalData::load: healthy={}, new={}, conflicts={}, leftover={}, stale={}, sidecars={}, sidecar_conflicts={}",
            healthy.len(), new.len(), conflicts.len(), leftover.len(), stale.len(), sidecars.len(), sidecar_conflicts.len(),
        ));

        // Aggregate new files by directory (using corpus_path), then merge sidecar counts
        let mut new_by_dir =
            Self::aggregate_by_directory(new.iter().map(|f| f.corpus_path.as_str()));
        Self::merge_sidecar_counts(&mut new_by_dir, &sidecars);

        // Aggregate leftover files by directory (using library_path)
        let leftover_by_dir =
            Self::aggregate_by_directory(leftover.iter().map(|f| f.library_path.as_str()));

        // Compute "replaced" leftovers: leftovers whose library_path matches a new deployment
        // New files deploy to "{library_name}/{deploy_path}", which matches leftover library_path format
        let new_destinations: HashSet<String> = new
            .iter()
            .filter(|f| !f.library_name.is_empty())
            .map(|f| format!("{}/{}", f.library_name, f.deploy_path))
            .collect();

        let replaced_count = leftover
            .iter()
            .filter(|f| new_destinations.contains(&f.library_path))
            .count();

        // Build per-library breakdown
        let per_library = Self::compute_per_library(
            &healthy,
            &new,
            &leftover,
            &stale,
            &conflicts,
            &new_destinations,
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
            sidecar_conflicts,
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

    /// Load precomputed sidecar deploy entries from signal table.
    ///
    /// Sidecar discovery is done by `DeriveCorpusDeployStatus` in the background
    /// computation pipeline. The modal just reads the results.
    fn load_sidecars(read_db: &ReadOnlyDb<'_>) -> Vec<SidecarDeployEntry> {
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

    /// Merge sidecar image counts into directory aggregates.
    ///
    /// For directories that already have audio files, increments sidecar_count.
    /// For directories with only sidecars (no audio), creates new entries.
    /// Re-sorts by total count (audio + sidecar) descending.
    fn merge_sidecar_counts(
        new_by_dir: &mut Vec<DirectoryAggregate>,
        sidecars: &[SidecarDeployEntry],
    ) {
        if sidecars.is_empty() {
            return;
        }

        // Count sidecars per corpus directory
        let mut sidecar_dir_counts: HashMap<String, usize> = HashMap::new();
        for s in sidecars {
            let dir = Path::new(&s.corpus_image_path)
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| "/".to_string());
            *sidecar_dir_counts.entry(dir).or_insert(0) += 1;
        }

        // Merge into existing entries or create new ones
        for entry in new_by_dir.iter_mut() {
            if let Some(sc_count) = sidecar_dir_counts.remove(&entry.directory) {
                entry.sidecar_count = sc_count;
            }
        }

        // Remaining are sidecar-only directories
        for (directory, sc_count) in sidecar_dir_counts {
            new_by_dir.push(DirectoryAggregate {
                directory,
                count: 0,
                sidecar_count: sc_count,
            });
        }

        // Re-sort by total count descending
        new_by_dir.sort_by(|a, b| (b.count + b.sidecar_count).cmp(&(a.count + a.sidecar_count)));
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
            .map(|(directory, count)| DirectoryAggregate {
                directory,
                count,
                sidecar_count: 0,
            })
            .collect();

        // Sort by count descending
        aggregates.sort_by(|a, b| b.count.cmp(&a.count));
        aggregates
    }

    /// Get counts for each tab (for display in tab bar).
    pub fn tab_counts(&self) -> [usize; 5] {
        [
            self.healthy.len(),
            self.new.len() + self.sidecars.len(),
            self.conflicts.len() + self.sidecar_conflicts.len(),
            self.leftover.len(),
            self.stale.len(),
        ]
    }

    /// Total operations that will be performed (excluding healthy).
    /// Conflict winners now appear in `new` via the computation layer.
    pub fn total_operations(&self) -> usize {
        self.new.len() + self.stale.len() + self.leftover.len() + self.sidecars.len()
    }

    /// Convert all deploy modal data into staged mutations.
    ///
    /// Ordering within `deploy`: leftovers first → stale → new
    /// (stash orphans before moving/linking into those paths).
    pub fn to_mutations(&self, resolver: &crate::corpus::paths::PathResolver) -> DeployMutationSet {
        let mut deploy = Vec::new();
        let mut skipped = 0usize;

        // 1. Leftovers: stash orphan files to clear destination paths
        for file in &self.leftover {
            deploy.push(file.to_mutation(resolver));
        }

        // 2. Stale: move existing deployments to correct paths
        for file in &self.stale {
            deploy.push(file.to_mutation(resolver));
        }

        // 3. New: deploy hard links (destinations now clear)
        for file in &self.new {
            match file.to_mutation(resolver) {
                Some(m) => deploy.push(m),
                None => skipped += 1,
            }
        }

        // 4. Sidecars: separate decision
        let sidecars: Vec<_> = self
            .sidecars
            .iter()
            .map(|s| s.to_mutation(resolver))
            .collect();

        DeployMutationSet {
            deploy,
            sidecars,
            skipped,
        }
    }
}
