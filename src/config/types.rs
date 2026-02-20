//! Configuration types: structs, enums, Default impls, and Config methods.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock, RwLockReadGuard};

use super::path_schema::PathTagSchema;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub root: PathBuf,
    pub legacy_enabled: bool,
    pub source_dirs: Vec<SourceDir>,
    pub opinions: Opinions,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Opinions {
    /// When true, lossy shit formats (MP3, M4A, etc.) are captured to FLAC
    /// instead of transcoded to Opus. The decoded PCM waveform is losslessly
    /// stored in a FLAC container with extension `.mp3.LOSSY.flac`.
    pub lossy_shit_formats_to_flac: bool,
    pub fingerprint_matching: FingerprintMatchingOpinions,
    pub quality_resolution: QualityResolutionOpinions,
    pub canonicalization: CanonicalizationOpinions,
    pub startup: StartupOpinions,
    pub health_detection: HealthDetectionOpinions,
    pub performance: PerformanceOpinions,
    pub tag_splitting: TagSplittingOpinions,
    pub duplicate_analysis: DuplicateAnalysisOpinions,
    pub inbox_organize: InboxOrganizeOpinions,
    /// Seconds of idle time before auto-rescanning corpus/inbox for filesystem changes.
    /// Default: 180. Set to 0 to disable.
    pub idle_rescan_interval_secs: u64,
    /// When true, keep one persistent transaction open across modal interactions.
    /// Decisions accumulate in a Transaction tab; commit/discard from there.
    /// Default: false.
    pub leave_transactions_open: bool,
}


/// Opinions for fingerprint matching thresholds
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FingerprintMatchingOpinions {
    /// Duration difference above this % = different track (default: 10.0)
    pub duration_tolerance_percent: f64,
    /// Same dir + different track# = not duplicate (default: true)
    pub require_matching_track_number: bool,
    /// Different albums can still be duplicates (default: false)
    pub require_matching_album: bool,
}

impl Default for FingerprintMatchingOpinions {
    fn default() -> Self {
        Self {
            duration_tolerance_percent: 1.0,
            require_matching_track_number: true,
            require_matching_album: false,
        }
    }
}

/// Opinions for quality-based auto-resolution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityResolutionOpinions {
    /// FLAC beats MP3 automatically (default: true)
    pub auto_resolve_format_tier: bool,
    /// Bitrate diff above this % = clear winner (default: 50.0)
    pub bitrate_threshold_percent: f64,
    /// Inbox-to-corpus bitrate fuzz tolerance as a percentage (default: 5.0).
    /// Files within this % bitrate difference (same format class and sample rate)
    /// are treated as equivalent rather than superior/inferior. Suppresses noise
    /// from minor FLAC compression differences across encoder versions.
    pub inbox_bitrate_fuzz_percent: f64,
}

impl Default for QualityResolutionOpinions {
    fn default() -> Self {
        Self {
            auto_resolve_format_tier: true,
            bitrate_threshold_percent: 50.0,
            inbox_bitrate_fuzz_percent: 5.0,
        }
    }
}

/// Opinions for tag canonicalization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanonicalizationOpinions {
    /// Strip EP/LP suffixes during album collision detection (default: false).
    /// When true, "Album EP" and "Album" normalize to the same key and collide.
    pub strip_album_format_suffixes: bool,
}

impl Default for CanonicalizationOpinions {
    fn default() -> Self {
        Self {
            strip_album_format_suffixes: false,
        }
    }
}

/// Which view to land on after startup progress completes.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
pub enum StartupView {
    #[default]
    Health,
    Search,
    Browser,
    Inbox,
}

/// Opinions for startup behavior
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartupOpinions {
    /// Force verification of all indexed files at startup, bypassing mtime optimization (default: false).
    /// Catches out-of-band tag changes (external tools modified tags) and corrupt files.
    /// Slower startup but ensures database matches reality.
    pub force_check_all_files_at_startup: bool,
    /// Free-page ratio threshold for prompting DB compaction (default: 0.1 = 10%).
    /// Set to 0.0 to disable.
    pub vacuum_threshold: f64,
    /// Which view to open after startup progress completes (default: Health).
    pub default_view: StartupView,
}

impl Default for StartupOpinions {
    fn default() -> Self {
        Self {
            force_check_all_files_at_startup: false,
            vacuum_threshold: 0.1,
            default_view: StartupView::default(),
        }
    }
}

/// Opinions for health detection behavior
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthDetectionOpinions {
    /// Tags that must be present on every track (default: title, album, artist, album_artist)
    pub required_tags: Vec<String>,
    /// When true, album_artist is only required on compilation albums (>1 distinct artist).
    /// Single-artist albums without album_artist won't be flagged. Default: true.
    pub album_artist_only_required_if_compilation: bool,
    /// Suffix appended to track title when tagging as single (default: " (Single)").
    pub single_album_suffix: String,
}

impl Default for HealthDetectionOpinions {
    fn default() -> Self {
        Self {
            required_tags: vec![
                "title".to_string(),
                "album".to_string(),
                "artist".to_string(),
                "album_artist".to_string(),
            ],
            album_artist_only_required_if_compilation: true,
            single_album_suffix: String::new(),
        }
    }
}

/// Opinions for performance tuning (threads, caches, instrumentation)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceOpinions {
    /// Number of worker threads. None = 2x logical cores (default).
    pub worker_threads: Option<usize>,
    /// SQLite page cache size per connection in MB (default: 256).
    pub db_cache_mb: u32,
    /// Enable timing instrumentation and stats display (default: false).
    /// When false, skips all atomic counter updates for better performance.
    pub timing_instrumentation: bool,
}

impl Default for PerformanceOpinions {
    fn default() -> Self {
        Self {
            worker_threads: None, // 2x cores
            db_cache_mb: 256,
            timing_instrumentation: false,
        }
    }
}

/// Opinions for detecting and splitting compound tag values.
///
/// Simplified structure for easier editing via the config UI:
/// - `collaboration_keywords`: Keywords like "feat", "ft", "vs" for artist collabs
/// - `tag_separators`: Per-tag separator lists (e.g., ARTIST: [";"], GENRE: [";", ","])
/// - `canonicalization_synonyms`: Substitutions during matching (e.g., "and" -> "&")
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TagSplittingOpinions {
    /// Collaboration keywords for artist tags (e.g., "feat", "ft", "featuring", "vs", "with").
    /// Used to detect featuring patterns like "Artist A feat. Artist B".
    pub collaboration_keywords: std::collections::HashSet<String>,

    /// Per-tag separator strings. Key is uppercase tag name (e.g., "ARTIST", "GENRE").
    /// Each tag has a list of separators to check in order (e.g., [";", ","]).
    pub tag_separators: std::collections::HashMap<String, Vec<String>>,

    /// Canonicalization synonyms: before checking for known values during
    /// "separator-if-known" matching, these substitutions are applied.
    /// e.g., "and" -> "&" allows "Simon and Garfunkel" to match "Simon & Garfunkel".
    pub canonicalization_synonyms: std::collections::HashMap<String, String>,
}

impl Default for TagSplittingOpinions {
    fn default() -> Self {
        let mut collaboration_keywords = std::collections::HashSet::new();
        collaboration_keywords.insert("feat".to_string());
        collaboration_keywords.insert("featuring".to_string());
        collaboration_keywords.insert("ft".to_string());
        collaboration_keywords.insert("with".to_string());
        collaboration_keywords.insert("vs".to_string());

        let mut tag_separators = std::collections::HashMap::new();
        tag_separators.insert("ARTIST".to_string(), vec![";".to_string()]);
        tag_separators.insert("GENRE".to_string(), vec![";".to_string()]);

        let mut canonicalization_synonyms = std::collections::HashMap::new();
        canonicalization_synonyms.insert("and".to_string(), "&".to_string());

        Self {
            collaboration_keywords,
            tag_separators,
            canonicalization_synonyms,
        }
    }
}

/// Opinions for fingerprint duplicate analysis.
///
/// Controls similarity thresholds and duration tolerance for detecting
/// true duplicates vs. different versions of tracks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DuplicateAnalysisOpinions {
    /// Fingerprint similarity threshold (0-100). Pairs below this are not duplicates.
    /// Default: 95.0
    pub fingerprint_similarity_threshold: f64,
    /// Duration tolerance in milliseconds. Tracks with duration difference above this
    /// are clustered separately. Default: 2000 (2 seconds)
    pub duration_tolerance_ms: i64,
    /// Max diverging directory keys to consider as cross-directory overlap (emit signal).
    /// e.g., bandcamp|indie = 2 keys, emit CrossSourceOverlap signal.
    /// Default: 2
    pub cross_directory_max_keys: usize,
    /// Min diverging directory keys to skip entirely (likely legitimate variants).
    /// e.g., 3+ keys in monstercat = skip (don't emit signal).
    /// Default: 3
    pub within_directory_min_keys: usize,
    /// Skip duplicate pairs where either title contains variant keywords (remix, live,
    /// acoustic, etc.) and the titles differ. Prevents false duplicate matches between
    /// different versions of the same track. Default: true
    pub elide_variant_titles: bool,
}

impl Default for DuplicateAnalysisOpinions {
    fn default() -> Self {
        Self {
            fingerprint_similarity_threshold: 95.0,
            duration_tolerance_ms: 2000,
            cross_directory_max_keys: 2,
            within_directory_min_keys: 3,
            elide_variant_titles: true,
        }
    }
}

/// Directory granularity for inbox organize workflow.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
pub enum InboxOrganizeGranularity {
    /// Walk to deepest directories containing audio files (default).
    #[default]
    Leaf,
    /// Iterate only direct children of inbox/.
    TopLevel,
}

/// Opinions for inbox organize workflow.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InboxOrganizeOpinions {
    /// How to group inbox directories for the organize workflow.
    pub directory_granularity: InboxOrganizeGranularity,
}

impl Default for InboxOrganizeOpinions {
    fn default() -> Self {
        Self {
            directory_granularity: InboxOrganizeGranularity::default(),
        }
    }
}

impl Default for Opinions {
    fn default() -> Self {
        Self {
            lossy_shit_formats_to_flac: false,
            fingerprint_matching: FingerprintMatchingOpinions::default(),
            quality_resolution: QualityResolutionOpinions::default(),
            canonicalization: CanonicalizationOpinions::default(),
            startup: StartupOpinions::default(),
            health_detection: HealthDetectionOpinions::default(),
            performance: PerformanceOpinions::default(),
            tag_splitting: TagSplittingOpinions::default(),
            duplicate_analysis: DuplicateAnalysisOpinions::default(),
            inbox_organize: InboxOrganizeOpinions::default(),
            idle_rescan_interval_secs: 180,
            leave_transactions_open: false,
        }
    }
}

/// A configured source directory within the corpus.
///
/// Source directories are the logical "collections" that files belong to.
/// They define where files deploy to and how duplicates between sources
/// should be resolved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceDir {
    /// Path relative to corpus root (e.g., "web/releases/bandcamp")
    pub path: PathBuf,
    /// Target library names for deployment (e.g., ["music", "soundtracks"])
    /// A single source can deploy to multiple libraries.
    pub libraries: Vec<String>,
    /// Whether duplicates from this source can be stashed when another source wins (default: true)
    pub can_stash_dupes: bool,
    /// Whether intra-source duplicates should be flagged (default: true).
    /// When false, duplicate groups entirely within this source are suppressed.
    pub interior_dupes: bool,
    /// Optional path-tag schema: expected file path structure expressed as tag placeholders.
    /// When set, files under this source dir are checked for path-tag agreement.
    pub path_schema: Option<PathTagSchema>,
}

/// Shared config wrapped in `Arc<RwLock<Config>>` for thread-safe read/write access.
///
/// The write lock is only held briefly during config updates (in Witch::tick()),
/// and since tick() and render() are sequential on the main thread, there is no
/// contention for UI reads.
pub type SharedConfig = Arc<RwLock<Config>>;

/// Convenience accessor for reading shared config without manual lock management.
pub fn read_shared_config(shared: &SharedConfig) -> RwLockReadGuard<'_, Config> {
    shared.read().expect("SharedConfig lock poisoned")
}

impl Config {
    /// Wrap this config in a shared Arc<RwLock> for centralized access.
    pub fn into_shared(self) -> SharedConfig {
        Arc::new(RwLock::new(self))
    }

    // =========================================================================
    // Derived directory accessors
    // =========================================================================

    /// Corpus directory: `<root>/corpus/`
    pub fn corpus_dir(&self) -> PathBuf {
        self.root.join("corpus")
    }

    /// Libraries directory: `<root>/libraries/`
    pub fn libraries_dir(&self) -> PathBuf {
        self.root.join("libraries")
    }

    /// Stash directory: `<root>/stash/`
    pub fn stash_dir(&self) -> PathBuf {
        self.root.join("stash")
    }

    pub fn inbox_dir(&self) -> PathBuf {
        self.root.join("inbox")
    }

    // =========================================================================
    // Source directory queries
    // =========================================================================

    /// Get all corpus paths that deploy to a specific library.
    ///
    /// Returns absolute paths to corpus directories that are configured
    /// to deploy to the given library name.
    pub fn get_corpus_paths_for_library(&self, library_name: &str) -> Vec<PathBuf> {
        let corpus_dir = self.corpus_dir();
        self.source_dirs
            .iter()
            .filter(|sd| sd.libraries.contains(&library_name.to_string()))
            .map(|sd| corpus_dir.join(&sd.path))
            .collect()
    }

    /// Check if a file path is under a configured source directory.
    ///
    /// Returns true if the path starts with any configured source directory.
    /// Paths are expected in relative format: `corpus/<relative-path>`.
    pub fn is_path_in_source(&self, path: &Path) -> bool {
        self.source_dirs.iter().any(|sd| {
            let prefix = Path::new("corpus").join(&sd.path);
            path.starts_with(&prefix)
        })
    }

    /// Get the source directory for a corpus path.
    ///
    /// Given a corpus path (relative, e.g., `corpus/web/releases/bandcamp/...`),
    /// returns the matching SourceDir. Returns None if not under any source.
    pub fn get_source_for_path(&self, corpus_path: &Path) -> Option<&SourceDir> {
        // Find the most specific (longest) matching source
        self.source_dirs
            .iter()
            .filter(|sd| {
                let prefix = Path::new("corpus").join(&sd.path);
                corpus_path.starts_with(&prefix)
            })
            .max_by_key(|sd| sd.path.as_os_str().len())
    }

    /// Get the target library names for a corpus path.
    ///
    /// Given a corpus path (relative, e.g., `corpus/web/releases/...`), returns
    /// the library names from the matching source directory.
    /// Returns empty vec if no source matches or source has no libraries configured.
    pub fn get_libraries_for_corpus_path(&self, corpus_path: &Path) -> Vec<String> {
        self.get_source_for_path(corpus_path)
            .map(|sd| sd.libraries.clone())
            .unwrap_or_default()
    }

    /// Get the source directory config for a relative corpus path.
    ///
    /// Given a path relative to corpus root (WITHOUT the "corpus/" prefix),
    /// e.g., `web/releases/bandcamp/Artist/Album/track.flac`, returns the
    /// matching SourceDir. Returns None if not under any configured source.
    ///
    /// This is used by cross-source overlap detection to classify files by source.
    pub fn get_source_for_relative_path(&self, relative_path: &Path) -> Option<&SourceDir> {
        // Find the most specific (longest) matching source
        self.source_dirs
            .iter()
            .filter(|sd| relative_path.starts_with(&sd.path))
            .max_by_key(|sd| sd.path.as_os_str().len())
    }

    /// Get the path-tag schema for a relative corpus path.
    ///
    /// Walks matching SourceDirs from most specific to least specific.
    /// Returns the first schema found (child overrides parent).
    pub fn get_schema_for_relative_path(&self, relative_path: &Path) -> Option<&PathTagSchema> {
        // Collect all matching source dirs, sorted longest path first.
        let mut matching: Vec<&SourceDir> = self.source_dirs
            .iter()
            .filter(|sd| relative_path.starts_with(&sd.path))
            .collect();
        matching.sort_by(|a, b| b.path.as_os_str().len().cmp(&a.path.as_os_str().len()));

        // Return first schema found (most specific wins).
        matching.iter().find_map(|sd| sd.path_schema.as_ref())
    }

    // =========================================================================
    // Validation
    // =========================================================================

    /// Validate that the archive root exists and all subdirectories are on the same filesystem.
    ///
    /// This check ensures:
    /// - Hard links will work (corpus -> libraries)
    /// - Atomic moves will work (corpus -> stash)
    ///
    /// Uses st_dev (device ID) comparison rather than creating test files.
    /// Runtime checks during corpus walking will detect nested mount points.
    pub fn validate_same_filesystem(&self) -> Result<()> {
        use std::os::unix::fs::MetadataExt;

        let corpus_dir = self.corpus_dir();
        let libraries_dir = self.libraries_dir();
        let stash_dir = self.stash_dir();

        // Validate root exists
        if !self.root.exists() {
            anyhow::bail!(
                "Validation failed: archive root does not exist\n\
                 Path: {:?}\n\
                 \n\
                 Please create this directory or update config.kdl",
                self.root
            );
        }

        // Validate subdirectories exist and collect device IDs
        let mut device_ids: Vec<(&str, &PathBuf, u64)> = Vec::new();

        // Check root first
        let root_dev = std::fs::metadata(&self.root)
            .with_context(|| format!("Failed to stat root directory: {:?}", self.root))?
            .dev();
        device_ids.push(("root", &self.root, root_dev));

        for (name, dir) in [("corpus", &corpus_dir), ("libraries", &libraries_dir), ("stash", &stash_dir)] {
            if !dir.exists() {
                anyhow::bail!(
                    "Validation failed: {} directory does not exist\n\
                     Path: {:?}\n\
                     \n\
                     Please create this directory under your archive root.",
                    name,
                    dir
                );
            }

            let dev = std::fs::metadata(dir)
                .with_context(|| format!("Failed to stat {} directory", name))?
                .dev();
            device_ids.push((name, dir, dev));
        }

        // Check all directories are on the same filesystem
        let (first_name, first_dir, first_dev) = device_ids[0];
        for (name, dir, dev) in &device_ids[1..] {
            if *dev != first_dev {
                anyhow::bail!(
                    "Validation failed: directories are on different filesystems\n\
                     \n\
                     {} ({:?}): device {}\n\
                     {} ({:?}): device {}\n\
                     \n\
                     Hard links and atomic moves require the same filesystem.\n\
                     All subdirectories must be on the same volume as the archive root.\n\
                     \n\
                     Note: Nested mount points within these directories will be detected\n\
                     at runtime and trigger read-only safety mode.",
                    first_name, first_dir, first_dev,
                    name, dir, dev
                );
            }
        }

        // Store the expected device ID for runtime checks
        crate::corpus::paths::set_expected_device_id(root_dev);

        crate::logging::log_general(format!(
            "Filesystem validation passed (device {})",
            root_dev
        ));
        Ok(())
    }

    /// Validate configuration.
    pub fn validate(&self) -> Result<()> {
        self.validate_same_filesystem()
            .context("Filesystem validation failed")?;

        // Validate source paths don't escape corpus
        let corpus_dir = self.corpus_dir();
        for source in &self.source_dirs {
            let full_path = corpus_dir.join(&source.path);
            if !full_path.starts_with(&corpus_dir) {
                anyhow::bail!("Source path escapes corpus directory: {:?}", source.path);
            }
        }

        Ok(())
    }
}
