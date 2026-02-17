//! Configuration Module
//!
//! KDL configuration parsing and path utilities.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock, RwLockReadGuard};

// Re-export utilities from mm-utils
pub use mm_utils::{
    get_config_dir, get_db_path, is_audio_extension,
    AUDIO_EXTENSIONS,
};

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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    Insights,
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
    /// Which view to open after startup progress completes (default: Insights).
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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

// =============================================================================
// Global Performance Config
// =============================================================================

static PERFORMANCE_CONFIG: OnceLock<PerformanceOpinions> = OnceLock::new();

/// Initialize the global performance config. Called once at startup.
pub fn init_performance_config(opinions: PerformanceOpinions) {
    let _ = PERFORMANCE_CONFIG.set(opinions);
}

/// Get the configured worker thread count.
/// Returns 2x logical cores if not configured or not initialized.
pub fn get_worker_thread_count() -> usize {
    let default_count = std::thread::available_parallelism()
        .map(|n| n.get() * 2)
        .unwrap_or(8);

    PERFORMANCE_CONFIG
        .get()
        .and_then(|p| p.worker_threads)
        .unwrap_or(default_count)
}

/// Get the configured DB cache size in KB (for SQLite PRAGMA cache_size).
/// Returns negative value as SQLite interprets negative as KB.
pub fn get_db_cache_kb() -> i64 {
    let mb = PERFORMANCE_CONFIG
        .get()
        .map(|p| p.db_cache_mb)
        .unwrap_or(256);

    // Convert MB to KB, return as negative (SQLite convention for KB)
    -(mb as i64 * 1024)
}

/// Check if timing instrumentation is enabled.
/// When false, stats collection is skipped entirely for better performance.
pub fn is_timing_enabled() -> bool {
    PERFORMANCE_CONFIG
        .get()
        .map(|p| p.timing_instrumentation)
        .unwrap_or(false)
}

/// A configured source directory within the corpus.
///
/// Source directories are the logical "collections" that files belong to.
/// They define where files deploy to and how duplicates between sources
/// should be resolved.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceDir {
    /// Path relative to corpus root (e.g., "web/releases/bandcamp")
    pub path: PathBuf,
    /// Target library names for deployment (e.g., ["music", "soundtracks"])
    /// A single source can deploy to multiple libraries.
    pub libraries: Vec<String>,
    /// Whether duplicates from this source can be stashed when another source wins (default: true)
    pub can_stash_dupes: bool,
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
    pub fn is_path_in_source(&self, path: &std::path::Path) -> bool {
        self.source_dirs.iter().any(|sd| {
            let prefix = std::path::Path::new("corpus").join(&sd.path);
            path.starts_with(&prefix)
        })
    }

    /// Get the source directory for a corpus path.
    ///
    /// Given a corpus path (relative, e.g., `corpus/web/releases/bandcamp/...`),
    /// returns the matching SourceDir. Returns None if not under any source.
    pub fn get_source_for_path(&self, corpus_path: &std::path::Path) -> Option<&SourceDir> {
        // Find the most specific (longest) matching source
        self.source_dirs
            .iter()
            .filter(|sd| {
                let prefix = std::path::Path::new("corpus").join(&sd.path);
                corpus_path.starts_with(&prefix)
            })
            .max_by_key(|sd| sd.path.as_os_str().len())
    }

    /// Get the target library names for a corpus path.
    ///
    /// Given a corpus path (relative, e.g., `corpus/web/releases/...`), returns
    /// the library names from the matching source directory.
    /// Returns empty vec if no source matches or source has no libraries configured.
    pub fn get_libraries_for_corpus_path(&self, corpus_path: &std::path::Path) -> Vec<String> {
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
    pub fn get_source_for_relative_path(&self, relative_path: &std::path::Path) -> Option<&SourceDir> {
        // Find the most specific (longest) matching source
        self.source_dirs
            .iter()
            .filter(|sd| relative_path.starts_with(&sd.path))
            .max_by_key(|sd| sd.path.as_os_str().len())
    }

    // =========================================================================
    // Validation
    // =========================================================================

    /// Validate that the archive root exists and all subdirectories are on the same filesystem.
    ///
    /// This check ensures:
    /// - Hard links will work (corpus → libraries)
    /// - Atomic moves will work (corpus → stash)
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

// =============================================================================
// Config Write-Back (Comment-Preserving KDL Modification)
// =============================================================================

/// Apply config edits to a KDL document in place, preserving comments and formatting.
///
/// Parses `original_kdl` into a `KdlDocument`, then for each opinion field that
/// differs between `old_config` and `new_config`, modifies only the changed node.
/// Returns the modified KDL text.
pub fn apply_config_edits_to_kdl(original_kdl: &str, old_config: &Config, new_config: &Config) -> Result<String> {
    let mut doc: kdl::KdlDocument = original_kdl.parse()
        .context("Failed to parse original KDL for edit")?;

    // Ensure "opinions" block exists
    if doc.get("opinions").is_none() {
        let mut node = kdl::KdlNode::new("opinions");
        node.set_children(kdl::KdlDocument::new());
        doc.nodes_mut().push(node);
    }

    let opinions_node = doc.get_mut("opinions").unwrap();
    let opinions_doc = opinions_node.ensure_children();

    // --- General opinions (direct children of "opinions") ---
    if new_config.opinions.lossy_shit_formats_to_flac != old_config.opinions.lossy_shit_formats_to_flac {
        set_or_create_bool_node(opinions_doc, "lossy-shit-formats-to-flac", new_config.opinions.lossy_shit_formats_to_flac);
    }

    // --- Startup ---
    let old_s = &old_config.opinions.startup;
    let new_s = &new_config.opinions.startup;
    if new_s.force_check_all_files_at_startup != old_s.force_check_all_files_at_startup
        || new_s.vacuum_threshold != old_s.vacuum_threshold
        || new_s.default_view != old_s.default_view
    {
        let startup = ensure_child_block(opinions_doc, "startup");
        if new_s.force_check_all_files_at_startup != old_s.force_check_all_files_at_startup {
            set_or_create_bool_node(startup, "force-check-all-files", new_s.force_check_all_files_at_startup);
        }
        if new_s.vacuum_threshold != old_s.vacuum_threshold {
            set_or_create_float_node(startup, "vacuum-threshold", new_s.vacuum_threshold);
        }
        if new_s.default_view != old_s.default_view {
            let view_str = match new_s.default_view {
                StartupView::Insights => "insights",
                StartupView::Search => "search",
                StartupView::Browser => "browser",
                StartupView::Inbox => "inbox",
            };
            set_or_create_string_node(startup, "default-view", view_str);
        }
    }

    // --- Fingerprint Matching ---
    let old_fm = &old_config.opinions.fingerprint_matching;
    let new_fm = &new_config.opinions.fingerprint_matching;
    if new_fm.duration_tolerance_percent != old_fm.duration_tolerance_percent
        || new_fm.require_matching_track_number != old_fm.require_matching_track_number
        || new_fm.require_matching_album != old_fm.require_matching_album
    {
        let block = ensure_child_block(opinions_doc, "fingerprint-matching");
        if new_fm.duration_tolerance_percent != old_fm.duration_tolerance_percent {
            set_or_create_float_node(block, "duration-tolerance-percent", new_fm.duration_tolerance_percent);
        }
        if new_fm.require_matching_track_number != old_fm.require_matching_track_number {
            set_or_create_bool_node(block, "require-matching-track-number", new_fm.require_matching_track_number);
        }
        if new_fm.require_matching_album != old_fm.require_matching_album {
            set_or_create_bool_node(block, "require-matching-album", new_fm.require_matching_album);
        }
    }

    // --- Quality Resolution ---
    let old_qr = &old_config.opinions.quality_resolution;
    let new_qr = &new_config.opinions.quality_resolution;
    if new_qr.auto_resolve_format_tier != old_qr.auto_resolve_format_tier
        || new_qr.bitrate_threshold_percent != old_qr.bitrate_threshold_percent
        || new_qr.inbox_bitrate_fuzz_percent != old_qr.inbox_bitrate_fuzz_percent
    {
        let block = ensure_child_block(opinions_doc, "quality-resolution");
        if new_qr.auto_resolve_format_tier != old_qr.auto_resolve_format_tier {
            set_or_create_bool_node(block, "auto-resolve-format-tier", new_qr.auto_resolve_format_tier);
        }
        if new_qr.bitrate_threshold_percent != old_qr.bitrate_threshold_percent {
            set_or_create_float_node(block, "bitrate-threshold-percent", new_qr.bitrate_threshold_percent);
        }
        if new_qr.inbox_bitrate_fuzz_percent != old_qr.inbox_bitrate_fuzz_percent {
            set_or_create_float_node(block, "inbox-bitrate-fuzz-percent", new_qr.inbox_bitrate_fuzz_percent);
        }
    }

    // --- Canonicalization ---
    if new_config.opinions.canonicalization.strip_album_format_suffixes != old_config.opinions.canonicalization.strip_album_format_suffixes {
        let block = ensure_child_block(opinions_doc, "canonicalization");
        set_or_create_bool_node(block, "strip-album-format-suffixes", new_config.opinions.canonicalization.strip_album_format_suffixes);
    }

    // --- Health Detection ---
    let old_hd = &old_config.opinions.health_detection;
    let new_hd = &new_config.opinions.health_detection;
    if new_hd.required_tags != old_hd.required_tags
        || new_hd.album_artist_only_required_if_compilation != old_hd.album_artist_only_required_if_compilation
        || new_hd.single_album_suffix != old_hd.single_album_suffix
    {
        let block = ensure_child_block(opinions_doc, "health-detection");
        if new_hd.required_tags != old_hd.required_tags {
            // Remove old node and create new one with all tag values
            block.nodes_mut().retain(|n| n.name().value() != "required-tags");
            let mut node = kdl::KdlNode::new("required-tags");
            for tag in &new_hd.required_tags {
                node.push(kdl::KdlEntry::new(kdl::KdlValue::String(tag.clone())));
            }
            block.nodes_mut().push(node);
        }
        if new_hd.album_artist_only_required_if_compilation != old_hd.album_artist_only_required_if_compilation {
            set_or_create_bool_node(block, "album-artist-only-required-if-compilation", new_hd.album_artist_only_required_if_compilation);
        }
        if new_hd.single_album_suffix != old_hd.single_album_suffix {
            set_or_create_string_node(block, "single-album-suffix", &new_hd.single_album_suffix);
        }
    }

    // --- Duplicate Analysis ---
    let old_da = &old_config.opinions.duplicate_analysis;
    let new_da = &new_config.opinions.duplicate_analysis;
    if new_da.fingerprint_similarity_threshold != old_da.fingerprint_similarity_threshold
        || new_da.duration_tolerance_ms != old_da.duration_tolerance_ms
        || new_da.cross_directory_max_keys != old_da.cross_directory_max_keys
        || new_da.within_directory_min_keys != old_da.within_directory_min_keys
        || new_da.elide_variant_titles != old_da.elide_variant_titles
    {
        let block = ensure_child_block(opinions_doc, "duplicate-analysis");
        if new_da.fingerprint_similarity_threshold != old_da.fingerprint_similarity_threshold {
            set_or_create_float_node(block, "fingerprint-similarity-threshold", new_da.fingerprint_similarity_threshold);
        }
        if new_da.duration_tolerance_ms != old_da.duration_tolerance_ms {
            set_or_create_int_node(block, "duration-tolerance-ms", new_da.duration_tolerance_ms);
        }
        if new_da.cross_directory_max_keys != old_da.cross_directory_max_keys {
            set_or_create_int_node(block, "cross-directory-max-keys", new_da.cross_directory_max_keys as i64);
        }
        if new_da.within_directory_min_keys != old_da.within_directory_min_keys {
            set_or_create_int_node(block, "within-directory-min-keys", new_da.within_directory_min_keys as i64);
        }
        if new_da.elide_variant_titles != old_da.elide_variant_titles {
            set_or_create_bool_node(block, "elide-variant-titles", new_da.elide_variant_titles);
        }
    }

    // --- Idle Rescan Interval ---
    if new_config.opinions.idle_rescan_interval_secs != old_config.opinions.idle_rescan_interval_secs {
        let dur = std::time::Duration::from_secs(new_config.opinions.idle_rescan_interval_secs);
        let formatted = humantime::format_duration(dur).to_string();
        set_or_create_string_node(opinions_doc, "idle-rescan-interval", &formatted);
    }

    // --- Leave Transactions Open ---
    if new_config.opinions.leave_transactions_open != old_config.opinions.leave_transactions_open {
        set_or_create_bool_node(opinions_doc, "leave-transactions-open", new_config.opinions.leave_transactions_open);
    }

    // --- Inbox Organize ---
    if new_config.opinions.inbox_organize.directory_granularity != old_config.opinions.inbox_organize.directory_granularity {
        let block = ensure_child_block(opinions_doc, "inbox-organize");
        let gran_str = match new_config.opinions.inbox_organize.directory_granularity {
            InboxOrganizeGranularity::Leaf => "leaf",
            InboxOrganizeGranularity::TopLevel => "top-level",
        };
        set_or_create_string_node(block, "directory-granularity", gran_str);
    }

    // --- Tag Splitting ---
    let old_ts = &old_config.opinions.tag_splitting;
    let new_ts = &new_config.opinions.tag_splitting;
    if new_ts.collaboration_keywords != old_ts.collaboration_keywords
        || new_ts.tag_separators != old_ts.tag_separators
        || new_ts.canonicalization_synonyms != old_ts.canonicalization_synonyms
    {
        let block = ensure_child_block(opinions_doc, "tag-splitting");

        // Write collab keywords if changed
        if new_ts.collaboration_keywords != old_ts.collaboration_keywords {
            block.nodes_mut().retain(|n| n.name().value() != "collab");
            let mut node = kdl::KdlNode::new("collab");
            let mut keywords: Vec<&String> = new_ts.collaboration_keywords.iter().collect();
            keywords.sort();
            for kw in keywords {
                node.push(kdl::KdlEntry::new(kdl::KdlValue::String(kw.clone())));
            }
            block.nodes_mut().push(node);
        }

        // Write synonyms if changed
        if new_ts.canonicalization_synonyms != old_ts.canonicalization_synonyms {
            block.nodes_mut().retain(|n| n.name().value() != "synonyms");
            let mut synonyms_node = kdl::KdlNode::new("synonyms");
            let synonyms_doc = synonyms_node.ensure_children();
            let mut pairs: Vec<(&String, &String)> = new_ts.canonicalization_synonyms.iter().collect();
            pairs.sort_by_key(|(k, _)| *k);
            for (from, to) in pairs {
                let mut syn_node = kdl::KdlNode::new(from.as_str());
                syn_node.push(kdl::KdlEntry::new(kdl::KdlValue::String(to.clone())));
                synonyms_doc.nodes_mut().push(syn_node);
            }
            block.nodes_mut().push(synonyms_node);
        }

        // Write tag separators if changed
        if new_ts.tag_separators != old_ts.tag_separators {
            // Remove old tag separator nodes (all non-collab, non-synonyms nodes)
            block.nodes_mut().retain(|n| {
                let name = n.name().value();
                name == "collab" || name == "synonyms"
            });
            let mut tags: Vec<(&String, &Vec<String>)> = new_ts.tag_separators.iter().collect();
            tags.sort_by_key(|(k, _)| *k);
            for (tag_name, seps) in tags {
                let mut tag_node = kdl::KdlNode::new(tag_name.to_lowercase().as_str());
                for sep in seps {
                    tag_node.push(kdl::KdlEntry::new(kdl::KdlValue::String(sep.clone())));
                }
                block.nodes_mut().push(tag_node);
            }
        }
    }

    // --- Performance ---
    let old_p = &old_config.opinions.performance;
    let new_p = &new_config.opinions.performance;
    if new_p.worker_threads != old_p.worker_threads
        || new_p.db_cache_mb != old_p.db_cache_mb
        || new_p.timing_instrumentation != old_p.timing_instrumentation
    {
        let block = ensure_child_block(opinions_doc, "performance");
        if new_p.worker_threads != old_p.worker_threads {
            match new_p.worker_threads {
                Some(n) => set_or_create_int_node(block, "worker-threads", n as i64),
                None => { block.nodes_mut().retain(|n| n.name().value() != "worker-threads"); }
            }
        }
        if new_p.db_cache_mb != old_p.db_cache_mb {
            set_or_create_int_node(block, "db-cache", new_p.db_cache_mb as i64);
        }
        if new_p.timing_instrumentation != old_p.timing_instrumentation {
            set_or_create_bool_node(block, "timing-instrumentation", new_p.timing_instrumentation);
        }
    }

    Ok(doc.to_string())
}

/// Write config edits to disk with comment-preserving KDL modification.
///
/// 1. Backs up existing config.kdl → config.kdl.bak
/// 2. Applies edits to original KDL text
/// 3. Writes modified KDL to config.kdl
pub fn write_config_to_disk(original_kdl: &str, old_config: &Config, new_config: &Config) -> Result<()> {
    let config_dir = get_config_dir()?;
    let config_path = config_dir.join("config.kdl");
    let backup_path = config_dir.join("config.kdl.bak");

    // Backup existing config
    if config_path.exists() {
        fs::copy(&config_path, &backup_path)
            .with_context(|| format!("Failed to backup config to {:?}", backup_path))?;
    }

    // Apply edits to KDL
    let modified_kdl = apply_config_edits_to_kdl(original_kdl, old_config, new_config)?;

    // Write modified KDL
    fs::write(&config_path, &modified_kdl)
        .with_context(|| format!("Failed to write config to {:?}", config_path))?;

    Ok(())
}

// KDL modification helpers

/// Get or create a child block node within a KdlDocument.
fn ensure_child_block<'a>(doc: &'a mut kdl::KdlDocument, name: &str) -> &'a mut kdl::KdlDocument {
    // Check if node exists, create if not
    if doc.get(name).is_none() {
        let mut node = kdl::KdlNode::new(name);
        node.set_children(kdl::KdlDocument::new());
        doc.nodes_mut().push(node);
    }
    let child_node = doc.get_mut(name).expect("ensure_child_block: node should exist after creation");
    child_node.ensure_children()
}

/// Set or create a bool-valued node within a KdlDocument.
///
/// Note: `clear_fmt()` is called on modified entries so the serializer
/// uses the new value instead of the cached representation.
fn set_or_create_bool_node(doc: &mut kdl::KdlDocument, name: &str, value: bool) {
    if let Some(node) = doc.get_mut(name) {
        if let Some(entry) = node.entries_mut().first_mut() {
            entry.set_value(kdl::KdlValue::Bool(value));
            entry.clear_fmt();
        } else {
            node.push(kdl::KdlEntry::new(kdl::KdlValue::Bool(value)));
        }
    } else {
        let mut node = kdl::KdlNode::new(name);
        node.push(kdl::KdlEntry::new(kdl::KdlValue::Bool(value)));
        doc.nodes_mut().push(node);
    }
}

/// Set or create a float-valued node within a KdlDocument.
fn set_or_create_float_node(doc: &mut kdl::KdlDocument, name: &str, value: f64) {
    if let Some(node) = doc.get_mut(name) {
        if let Some(entry) = node.entries_mut().first_mut() {
            entry.set_value(kdl::KdlValue::Base10Float(value));
            entry.clear_fmt();
        } else {
            node.push(kdl::KdlEntry::new(kdl::KdlValue::Base10Float(value)));
        }
    } else {
        let mut node = kdl::KdlNode::new(name);
        node.push(kdl::KdlEntry::new(kdl::KdlValue::Base10Float(value)));
        doc.nodes_mut().push(node);
    }
}

/// Set or create an integer-valued node within a KdlDocument.
fn set_or_create_int_node(doc: &mut kdl::KdlDocument, name: &str, value: i64) {
    if let Some(node) = doc.get_mut(name) {
        if let Some(entry) = node.entries_mut().first_mut() {
            entry.set_value(kdl::KdlValue::Base10(value));
            entry.clear_fmt();
        } else {
            node.push(kdl::KdlEntry::new(kdl::KdlValue::Base10(value)));
        }
    } else {
        let mut node = kdl::KdlNode::new(name);
        node.push(kdl::KdlEntry::new(kdl::KdlValue::Base10(value)));
        doc.nodes_mut().push(node);
    }
}

/// Set or create a string-valued node within a KdlDocument.
fn set_or_create_string_node(doc: &mut kdl::KdlDocument, name: &str, value: &str) {
    if let Some(node) = doc.get_mut(name) {
        if let Some(entry) = node.entries_mut().first_mut() {
            entry.set_value(kdl::KdlValue::String(value.to_string()));
            entry.clear_fmt();
        } else {
            node.push(kdl::KdlEntry::new(kdl::KdlValue::String(value.to_string())));
        }
    } else {
        let mut node = kdl::KdlNode::new(name);
        node.push(kdl::KdlEntry::new(kdl::KdlValue::String(value.to_string())));
        doc.nodes_mut().push(node);
    }
}

/// Check whether a config file exists on disk.
///
/// Returns true if config.kdl is present in the config directory.
/// Used by first-time setup to determine if the wizard should run.
pub fn config_exists() -> bool {
    get_config_dir()
        .map(|dir| dir.join("config.kdl").exists())
        .unwrap_or(false)
}

/// Write a minimal initial config file with just the archive root path.
///
/// Used during first-time setup before the full config system is available.
/// The resulting config.kdl contains only the `root` directive; all other
/// settings use defaults.
pub fn write_initial_config(root: &Path) -> Result<()> {
    let config_dir = get_config_dir()?;
    fs::create_dir_all(&config_dir)?;
    let config_path = config_dir.join("config.kdl");

    let content = format!(
        "// Music Magic configuration\n\
         // See docs/ for full configuration reference.\n\
         \n\
         root \"{}\"\n",
        root.display()
    );

    fs::write(&config_path, content)
        .with_context(|| format!("Failed to write config to {:?}", config_path))?;

    Ok(())
}

/// Load config from disk. Does NOT validate.
///
/// Filesystem validation (`config.validate()`) should be called once at startup.
/// It doesn't need to be repeated - the filesystem layout won't change at runtime.
pub fn load_config() -> Result<Config> {
    let config_dir = get_config_dir()?;
    let config_path = config_dir.join("config.kdl");

    if !config_path.exists() {
        anyhow::bail!(
            "config.kdl not found at {:?}\n\
            Please create a config file at $XDG_CONFIG_HOME/mm/config.kdl (or ~/.config/mm/config.kdl)",
            config_path
        );
    }

    let content = fs::read_to_string(&config_path)
        .with_context(|| format!("Failed to read config from {:?}", config_path))?;

    parse_kdl_config(&content)
}

/// Parse fingerprint-matching opinions from KDL node
fn parse_fingerprint_matching_opinions(node: &kdl::KdlNode, opinions: &mut FingerprintMatchingOpinions) {
    if let Some(children) = node.children() {
        for child in children.nodes() {
            match child.name().value() {
                "duration-tolerance-percent" => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_f64() {
                            opinions.duration_tolerance_percent = val;
                        }
                    }
                }
                "require-matching-track-number" => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_bool() {
                            opinions.require_matching_track_number = val;
                        }
                    }
                }
                "require-matching-album" => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_bool() {
                            opinions.require_matching_album = val;
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

/// Parse quality-resolution opinions from KDL node
fn parse_quality_resolution_opinions(node: &kdl::KdlNode, opinions: &mut QualityResolutionOpinions) {
    if let Some(children) = node.children() {
        for child in children.nodes() {
            match child.name().value() {
                "auto-resolve-format-tier" => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_bool() {
                            opinions.auto_resolve_format_tier = val;
                        }
                    }
                }
                "bitrate-threshold-percent" => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_f64() {
                            opinions.bitrate_threshold_percent = val;
                        }
                    }
                }
                "inbox-bitrate-fuzz-percent" => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_f64() {
                            opinions.inbox_bitrate_fuzz_percent = val;
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

/// Parse canonicalization opinions from KDL node
fn parse_canonicalization_opinions(node: &kdl::KdlNode, opinions: &mut CanonicalizationOpinions) {
    if let Some(children) = node.children() {
        for child in children.nodes() {
            match child.name().value() {
                "strip-album-format-suffixes" => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_bool() {
                            opinions.strip_album_format_suffixes = val;
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

/// Parse startup opinions from KDL node
fn parse_startup_opinions(node: &kdl::KdlNode, opinions: &mut StartupOpinions) {
    if let Some(children) = node.children() {
        for child in children.nodes() {
            match child.name().value() {
                "force-check-all-files" => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_bool() {
                            opinions.force_check_all_files_at_startup = val;
                        }
                    }
                }
                "vacuum-threshold" => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_f64() {
                            opinions.vacuum_threshold = val;
                        }
                    }
                }
                "default-view" => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_string() {
                            match val {
                                "insights" => opinions.default_view = StartupView::Insights,
                                "search" => opinions.default_view = StartupView::Search,
                                "browser" => opinions.default_view = StartupView::Browser,
                                "inbox" => opinions.default_view = StartupView::Inbox,
                                _ => {}
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

/// Parse health detection opinions from KDL node
fn parse_health_detection_opinions(node: &kdl::KdlNode, opinions: &mut HealthDetectionOpinions) {
    if let Some(children) = node.children() {
        for child in children.nodes() {
            match child.name().value() {
                "required-tags" => {
                    let tags: Vec<String> = child
                        .entries()
                        .iter()
                        .filter_map(|e| e.value().as_string().map(|s| s.to_string()))
                        .collect();
                    if !tags.is_empty() {
                        opinions.required_tags = tags;
                    }
                }
                "album-artist-only-required-if-compilation" => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_bool() {
                            opinions.album_artist_only_required_if_compilation = val;
                        }
                    }
                }
                "single-album-suffix" => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_string() {
                            opinions.single_album_suffix = val.to_string();
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

/// Parse a human-readable size string like "256mb", "1gb", "512" into MB.
/// Accepts: plain numbers (interpreted as MB), or suffixed with kb/mb/gb (case-insensitive).
fn parse_size_mb(s: &str) -> Option<u32> {
    let s = s.trim().to_lowercase();

    if let Some(num_str) = s.strip_suffix("gb") {
        num_str.trim().parse::<u32>().ok().map(|n| n * 1024)
    } else if let Some(num_str) = s.strip_suffix("mb") {
        num_str.trim().parse::<u32>().ok()
    } else if let Some(num_str) = s.strip_suffix("kb") {
        // KB rounds up to nearest MB (minimum 1MB)
        num_str.trim().parse::<u32>().ok().map(|n| n.div_ceil(1024))
    } else {
        // Plain number = MB
        s.parse::<u32>().ok()
    }
}

/// Parse performance opinions from KDL node
fn parse_performance_opinions(node: &kdl::KdlNode, opinions: &mut PerformanceOpinions) {
    if let Some(children) = node.children() {
        for child in children.nodes() {
            match child.name().value() {
                "worker-threads" => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_i64() {
                            if val > 0 {
                                opinions.worker_threads = Some(val as usize);
                            }
                        }
                    }
                }
                "db-cache" => {
                    if let Some(entry) = child.entries().first() {
                        // Try string first (e.g., "256mb", "1gb")
                        if let Some(s) = entry.value().as_string() {
                            if let Some(mb) = parse_size_mb(s) {
                                opinions.db_cache_mb = mb;
                            }
                        }
                        // Fall back to plain integer (interpreted as MB)
                        else if let Some(val) = entry.value().as_i64() {
                            if val > 0 {
                                opinions.db_cache_mb = val as u32;
                            }
                        }
                    }
                }
                "timing-instrumentation" => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_bool() {
                            opinions.timing_instrumentation = val;
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

/// Parse tag-splitting opinions from KDL node.
///
/// Expected format:
/// ```kdl
/// tag-splitting {
///     collab "feat" "featuring" "ft" "with" "vs"
///     synonyms {
///         "and" "&"
///     }
///     artist ";"
///     genre ";" ","
/// }
/// ```
///
/// - `collab` node: list of collaboration keywords (replaces defaults if present)
/// - `synonyms` node: key-value pairs for canonicalization substitutions
/// - Other nodes: tag name with list of separator strings
fn parse_tag_splitting_opinions(node: &kdl::KdlNode, opinions: &mut TagSplittingOpinions) {
    if let Some(children) = node.children() {
        for child in children.nodes() {
            let node_name = child.name().value();

            match node_name {
                "collab" => {
                    // Parse collaboration keywords
                    let keywords: std::collections::HashSet<String> = child
                        .entries()
                        .iter()
                        .filter_map(|e| e.value().as_string().map(|s| s.to_string()))
                        .collect();
                    if !keywords.is_empty() {
                        opinions.collaboration_keywords = keywords;
                    }
                }
                "synonyms" => {
                    // Parse synonyms block: each child node is "from" "to"
                    if let Some(synonym_nodes) = child.children() {
                        let mut synonyms = std::collections::HashMap::new();
                        for synonym_node in synonym_nodes.nodes() {
                            let from = synonym_node.name().value().to_string();
                            if let Some(entry) = synonym_node.entries().first() {
                                if let Some(to) = entry.value().as_string() {
                                    synonyms.insert(from, to.to_string());
                                }
                            }
                        }
                        if !synonyms.is_empty() {
                            opinions.canonicalization_synonyms = synonyms;
                        }
                    }
                }
                _ => {
                    // Treat as tag name with separator list
                    let tag_name = node_name.to_uppercase();
                    let separators: Vec<String> = child
                        .entries()
                        .iter()
                        .filter_map(|e| e.value().as_string().map(|s| s.to_string()))
                        .collect();
                    if !separators.is_empty() {
                        opinions.tag_separators.insert(tag_name, separators);
                    }
                }
            }
        }
    }
}

/// Parse duplicate-analysis opinions from KDL node
fn parse_duplicate_analysis_opinions(node: &kdl::KdlNode, opinions: &mut DuplicateAnalysisOpinions) {
    if let Some(children) = node.children() {
        for child in children.nodes() {
            match child.name().value() {
                "fingerprint-similarity-threshold" => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_f64() {
                            opinions.fingerprint_similarity_threshold = val;
                        }
                    }
                }
                "duration-tolerance-ms" => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_i64() {
                            opinions.duration_tolerance_ms = val;
                        }
                    }
                }
                "cross-directory-max-keys" => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_i64() {
                            if val > 0 {
                                opinions.cross_directory_max_keys = val as usize;
                            }
                        }
                    }
                }
                "within-directory-min-keys" => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_i64() {
                            if val > 0 {
                                opinions.within_directory_min_keys = val as usize;
                            }
                        }
                    }
                }
                "elide-variant-titles" => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_bool() {
                            opinions.elide_variant_titles = val;
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

/// Parse inbox-organize opinions from KDL node
fn parse_inbox_organize_opinions(node: &kdl::KdlNode, opinions: &mut InboxOrganizeOpinions) {
    if let Some(children) = node.children() {
        for child in children.nodes() {
            match child.name().value() {
                "directory-granularity" => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_string() {
                            match val {
                                "leaf" => opinions.directory_granularity = InboxOrganizeGranularity::Leaf,
                                "top-level" => opinions.directory_granularity = InboxOrganizeGranularity::TopLevel,
                                _ => {}
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

fn parse_kdl_config(content: &str) -> Result<Config> {
    let doc: kdl::KdlDocument = content.parse().context("Failed to parse KDL document")?;

    let mut config = Config {
        root: PathBuf::new(),
        legacy_enabled: false,
        source_dirs: Vec::new(),
        opinions: Opinions::default(),
    };

    for node in doc.nodes() {
        match node.name().value() {
            "root" => {
                if let Some(path) = node.entries().first() {
                    if let Some(path_str) = path.value().as_string() {
                        config.root = PathBuf::from(path_str);
                    }
                }
            }
            "legacy-library" => {
                // Boolean toggle: `legacy-library true`
                if let Some(entry) = node.entries().first() {
                    if let Some(val) = entry.value().as_bool() {
                        config.legacy_enabled = val;
                    }
                }
            }
            "dir" => {
                // dir "web/releases/bandcamp" { library "music" "soundtracks"; can-stash-dupes false }
                let path = node.entries().first()
                    .and_then(|e| e.value().as_string())
                    .map(PathBuf::from);

                if let Some(path) = path {
                    let mut source = SourceDir {
                        path,
                        libraries: Vec::new(),
                        can_stash_dupes: true, // default true
                    };

                    if let Some(children) = node.children() {
                        for child in children.nodes() {
                            match child.name().value() {
                                "library" => {
                                    // Collect all string values from this library node
                                    for entry in child.entries() {
                                        if let Some(s) = entry.value().as_string() {
                                            source.libraries.push(s.to_string());
                                        }
                                    }
                                }
                                "can-stash-dupes" => {
                                    if let Some(entry) = child.entries().first() {
                                        source.can_stash_dupes = entry.value().as_bool().unwrap_or(true);
                                    }
                                }
                                _ => {}
                            }
                        }
                    }

                    config.source_dirs.push(source);
                }
            }
            "opinions" => {
                if let Some(children) = node.children() {
                    for child in children.nodes() {
                        match child.name().value() {
                            "lossy-shit-formats-to-flac" => {
                                if let Some(entry) = child.entries().first() {
                                    if let Some(val) = entry.value().as_bool() {
                                        config.opinions.lossy_shit_formats_to_flac = val;
                                    }
                                }
                            }
                            "fingerprint-matching" => {
                                parse_fingerprint_matching_opinions(child, &mut config.opinions.fingerprint_matching);
                            }
                            "quality-resolution" => {
                                parse_quality_resolution_opinions(child, &mut config.opinions.quality_resolution);
                            }
                            "canonicalization" => {
                                parse_canonicalization_opinions(child, &mut config.opinions.canonicalization);
                            }
                            "startup" => {
                                parse_startup_opinions(child, &mut config.opinions.startup);
                            }
                            "health-detection" => {
                                parse_health_detection_opinions(child, &mut config.opinions.health_detection);
                            }
                            "performance" => {
                                parse_performance_opinions(child, &mut config.opinions.performance);
                            }
                            "tag-splitting" => {
                                parse_tag_splitting_opinions(child, &mut config.opinions.tag_splitting);
                            }
                            "duplicate-analysis" => {
                                parse_duplicate_analysis_opinions(child, &mut config.opinions.duplicate_analysis);
                            }
                            "inbox-organize" => {
                                parse_inbox_organize_opinions(child, &mut config.opinions.inbox_organize);
                            }
                            "idle-rescan-interval" => {
                                if let Some(entry) = child.entries().first() {
                                    if let Some(s) = entry.value().as_string() {
                                        if let Ok(dur) = humantime::parse_duration(s) {
                                            config.opinions.idle_rescan_interval_secs = dur.as_secs();
                                        }
                                    } else if let Some(val) = entry.value().as_i64() {
                                        // Legacy: bare integer seconds
                                        config.opinions.idle_rescan_interval_secs = val.max(0) as u64;
                                    }
                                }
                            }
                            "leave-transactions-open" => {
                                if let Some(entry) = child.entries().first() {
                                    if let Some(val) = entry.value().as_bool() {
                                        config.opinions.leave_transactions_open = val;
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            _ => {}
        }
    }

    if config.root.as_os_str().is_empty() {
        anyhow::bail!("root not specified in config.kdl");
    }

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_single_root() {
        let kdl = r#"
root "/Volumes/cerberus/archive"

dir "web/releases/bandcamp" {
    library "music"
    can-stash-dupes false
}

dir "web/releases/indie" {
    library "music" "soundtracks"
}

legacy-library true
"#;

        let config = parse_kdl_config(kdl).unwrap();
        assert_eq!(config.root, PathBuf::from("/Volumes/cerberus/archive"));
        assert_eq!(config.corpus_dir(), PathBuf::from("/Volumes/cerberus/archive/corpus"));
        assert_eq!(config.libraries_dir(), PathBuf::from("/Volumes/cerberus/archive/libraries"));
        assert_eq!(config.stash_dir(), PathBuf::from("/Volumes/cerberus/archive/stash"));
        assert!(config.legacy_enabled);
        assert_eq!(config.source_dirs.len(), 2);
        assert_eq!(config.source_dirs[0].path, PathBuf::from("web/releases/bandcamp"));
        assert_eq!(config.source_dirs[0].libraries, vec!["music".to_string()]);
        assert!(!config.source_dirs[0].can_stash_dupes); // explicitly false
        assert_eq!(config.source_dirs[1].path, PathBuf::from("web/releases/indie"));
        assert_eq!(config.source_dirs[1].libraries, vec!["music".to_string(), "soundtracks".to_string()]);
        assert!(config.source_dirs[1].can_stash_dupes); // default true
    }

    #[test]
    fn test_missing_root() {
        let kdl = r#"
legacy-library true
"#;

        let result = parse_kdl_config(kdl);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("root not specified"));
    }

    #[test]
    fn test_legacy_disabled_by_default() {
        let kdl = r#"
root "/archive"
"#;

        let config = parse_kdl_config(kdl).unwrap();
        assert!(!config.legacy_enabled);
    }

    #[test]
    fn test_opinions_parsing() {
        let kdl = r#"
root "/archive"

opinions {
    fingerprint-matching {
        duration-tolerance-percent 15.0
        require-matching-track-number false
        require-matching-album true
    }

    quality-resolution {
        auto-resolve-format-tier false
        bitrate-threshold-percent 75.0
        inbox-bitrate-fuzz-percent 3.0
    }

    canonicalization {
        strip-album-format-suffixes false
    }

    re-releases {
        same-fingerprint-different-album "flag"
    }
}
"#;

        let config = parse_kdl_config(kdl).unwrap();

        assert_eq!(config.opinions.fingerprint_matching.duration_tolerance_percent, 15.0);
        assert!(!config.opinions.fingerprint_matching.require_matching_track_number);
        assert!(config.opinions.fingerprint_matching.require_matching_album);
        assert!(!config.opinions.quality_resolution.auto_resolve_format_tier);
        assert_eq!(config.opinions.quality_resolution.bitrate_threshold_percent, 75.0);
        assert_eq!(config.opinions.quality_resolution.inbox_bitrate_fuzz_percent, 3.0);
        assert!(!config.opinions.canonicalization.strip_album_format_suffixes);
    }

    #[test]
    fn test_opinions_defaults() {
        let kdl = r#"
root "/archive"
"#;

        let config = parse_kdl_config(kdl).unwrap();

        assert!(!config.opinions.lossy_shit_formats_to_flac);
        assert_eq!(config.opinions.fingerprint_matching.duration_tolerance_percent, 1.0);
        assert!(config.opinions.fingerprint_matching.require_matching_track_number);
        assert!(!config.opinions.fingerprint_matching.require_matching_album);
        assert!(config.opinions.quality_resolution.auto_resolve_format_tier);
        assert_eq!(config.opinions.quality_resolution.bitrate_threshold_percent, 50.0);
        assert_eq!(config.opinions.quality_resolution.inbox_bitrate_fuzz_percent, 5.0);
        assert!(!config.opinions.canonicalization.strip_album_format_suffixes);
    }

    #[test]
    fn test_lossy_shit_formats_to_flac_opinion() {
        let kdl = r#"
root "/archive"

opinions {
    lossy-shit-formats-to-flac true
}
"#;

        let config = parse_kdl_config(kdl).unwrap();
        assert!(config.opinions.lossy_shit_formats_to_flac);

        // Explicit false
        let kdl_false = r#"
root "/archive"

opinions {
    lossy-shit-formats-to-flac false
}
"#;
        let config_false = parse_kdl_config(kdl_false).unwrap();
        assert!(!config_false.opinions.lossy_shit_formats_to_flac);
    }

    #[test]
    fn test_is_path_in_source() {
        let kdl = r#"
root "/archive"

dir "web/releases/bandcamp" {
    library "music"
}

dir "web/releases/steam" {
    library "soundtracks"
}
"#;

        let config = parse_kdl_config(kdl).unwrap();

        // Relative paths (as stored in DB) should match
        assert!(config.is_path_in_source(std::path::Path::new(
            "corpus/web/releases/bandcamp/Artist/Album/track.flac"
        )));
        assert!(config.is_path_in_source(std::path::Path::new(
            "corpus/web/releases/steam/Game/Soundtrack/01.mp3"
        )));

        // Non-configured paths should not match
        assert!(!config.is_path_in_source(std::path::Path::new(
            "corpus/web/releases/itunes/Artist/Album/track.flac"
        )));
        assert!(!config.is_path_in_source(std::path::Path::new(
            "corpus/physical/cd/Artist/Album/track.flac"
        )));

        // Exact prefix match (not substring)
        assert!(!config.is_path_in_source(std::path::Path::new(
            "corpus/web/releases/bandcamp-extra/Artist/track.flac"
        )));
    }

    #[test]
    fn test_kdl_direct_modification() {
        let kdl_str = r#"root "/archive"

opinions {
    startup {
        default-view "insights"
    }
}
"#;
        let mut doc: kdl::KdlDocument = kdl_str.parse().unwrap();

        // Navigate: opinions -> startup -> default-view
        let opinions = doc.get_mut("opinions").unwrap();
        let opinions_children = opinions.ensure_children();
        let startup = opinions_children.get_mut("startup").unwrap();
        let startup_children = startup.ensure_children();
        let dv = startup_children.get_mut("default-view").unwrap();
        let entry = dv.entries_mut().first_mut().unwrap();
        entry.set_value(kdl::KdlValue::String("browser".to_string()));
        entry.clear_fmt(); // Required: clear cached repr so new value serializes

        let result = doc.to_string();
        assert!(result.contains("browser"), "Should contain 'browser': {}", result);
    }

    #[test]
    fn test_apply_config_edits_preserves_unchanged() {
        let kdl = r#"root "/archive"

// Important comment
dir "web/releases/bandcamp" {
    library "music"
}

opinions {
    startup {
        default-view "insights"
    }
}
"#;
        let config = parse_kdl_config(kdl).unwrap();
        // Apply with no changes — should round-trip
        let result = apply_config_edits_to_kdl(kdl, &config, &config).unwrap();
        // Re-parse the result and verify it's equivalent
        let reparsed = parse_kdl_config(&result).unwrap();
        assert_eq!(reparsed.root, config.root);
        assert_eq!(reparsed.opinions.startup.default_view, config.opinions.startup.default_view);
    }

    #[test]
    fn test_apply_config_edits_changes_single_field() {
        let kdl = r#"root "/archive"

opinions {
    startup {
        default-view "insights"
    }
}
"#;
        let old_config = parse_kdl_config(kdl).unwrap();
        let mut new_config = old_config.clone();
        new_config.opinions.startup.default_view = StartupView::Browser;

        let result = apply_config_edits_to_kdl(kdl, &old_config, &new_config).unwrap();
        // Verify the KDL text contains "browser"
        assert!(result.contains("browser"), "Result KDL should contain 'browser': {}", result);
        let reparsed = parse_kdl_config(&result).unwrap();
        assert_eq!(reparsed.opinions.startup.default_view, StartupView::Browser);
        // Root should be preserved
        assert_eq!(reparsed.root, PathBuf::from("/archive"));
    }

    #[test]
    fn test_apply_config_edits_inserts_new_block() {
        let kdl = r#"root "/archive"
"#;
        let old_config = parse_kdl_config(kdl).unwrap();
        let mut new_config = old_config.clone();
        new_config.opinions.quality_resolution.bitrate_threshold_percent = 75.0;

        let result = apply_config_edits_to_kdl(kdl, &old_config, &new_config).unwrap();
        let reparsed = parse_kdl_config(&result).unwrap();
        assert_eq!(reparsed.opinions.quality_resolution.bitrate_threshold_percent, 75.0);
    }

    #[test]
    fn test_apply_config_edits_bool_toggle() {
        let kdl = r#"root "/archive"

opinions {
    fingerprint-matching {
        require-matching-album false
    }
}
"#;
        let old_config = parse_kdl_config(kdl).unwrap();
        let mut new_config = old_config.clone();
        new_config.opinions.fingerprint_matching.require_matching_album = true;

        let result = apply_config_edits_to_kdl(kdl, &old_config, &new_config).unwrap();
        let reparsed = parse_kdl_config(&result).unwrap();
        assert!(reparsed.opinions.fingerprint_matching.require_matching_album);
    }

    #[test]
    fn test_startup_default_view_parsing() {
        // Default is Insights when not specified
        let kdl = r#"root "/archive""#;
        let config = parse_kdl_config(kdl).unwrap();
        assert_eq!(config.opinions.startup.default_view, StartupView::Insights);

        // Explicit values
        for (value, expected) in [
            ("insights", StartupView::Insights),
            ("search", StartupView::Search),
            ("browser", StartupView::Browser),
            ("inbox", StartupView::Inbox),
        ] {
            let kdl = format!(
                r#"root "/archive"
opinions {{
    startup {{
        default-view "{value}"
    }}
}}"#
            );
            let config = parse_kdl_config(&kdl).unwrap();
            assert_eq!(
                config.opinions.startup.default_view, expected,
                "default-view \"{value}\" should parse to {expected:?}"
            );
        }
    }
}
