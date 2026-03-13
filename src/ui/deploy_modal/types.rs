//! Deploy Modal Types
//!
//! Data structures for the deploy modal, including cached signal data
//! and per-library breakdown.

use crate::meta::views::{
    ConflictGroup, DeploySignalFile, LeftoverSignalFile, SidecarConflictGroup, StaleSignalFile,
};

/// Directory aggregate for grouped file display.
/// Sorted by count descending.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DirectoryAggregate {
    /// Directory path
    pub directory: String,
    /// Number of audio files in this directory
    pub count: usize,
    /// Number of sidecar images in this directory
    pub sidecar_count: usize,
}

/// Per-library breakdown of deploy operations.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
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
