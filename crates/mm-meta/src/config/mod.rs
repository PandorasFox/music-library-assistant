//! Configuration types: structs, enums, Default impls.
//!
//! Pure data types and query methods. Validation that touches the filesystem
//! (validate_same_filesystem, set_expected_device_id) stays in the mm crate.

pub mod path_schema;

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock, RwLockReadGuard};

pub use path_schema::PathTagSchema;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub root: PathBuf,
    pub legacy_enabled: bool,
    pub source_dirs: Vec<SourceDir>,
    pub opinions: Opinions,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Opinions {
    pub quality_resolution: QualityResolutionOpinions,
    pub canonicalization: CanonicalizationOpinions,
    pub startup: StartupOpinions,
    pub health_detection: HealthDetectionOpinions,
    pub performance: PerformanceOpinions,
    pub tag_splitting: TagSplittingOpinions,
    pub duplicate_analysis: DuplicateAnalysisOpinions,
    pub release_packing: ReleasePackingOpinions,
    pub inbox_organize: InboxOrganizeOpinions,
    /// When true, keep one persistent transaction open across modal interactions.
    /// Decisions accumulate in a Transaction tab; commit/discard from there.
    /// Default: false.
    pub leave_transactions_open: bool,
    /// External matching (AcoustID, etc.) configuration.
    pub external_matching: ExternalMatchingConfig,
    /// Disc extraction configuration (tag name, letter mapping).
    pub disc_extraction: DiscExtractionOpinions,
    /// Album art embedding/upgrade configuration.
    pub album_art: AlbumArtOpinions,
    /// Debug/diagnostic options (reserved, currently empty).
    pub debug: DebugOpinions,
    /// Poll interval in seconds for the filesystem watcher fallback mode.
    /// Used when inotify is unavailable (watch limit hit, creation failure).
    /// Default: 900 (15 minutes).
    pub watcher_poll_interval_secs: u64,
    /// Session lifetime in days. None = sessions expire on process exit ("close"),
    /// Some(n) = sessions expire after n days. Default: Some(30).
    pub session_lifetime_days: Option<u64>,
}

/// KDL field names — single source of truth for parse/edit/source-detection.
impl Opinions {
    // Direct children of the "opinions" block
    pub const KDL_LEAVE_TXN_OPEN: &str = "leave-transactions-open";

    pub const KDL_WATCHER_POLL_INTERVAL: &str = "watcher-poll-interval-secs";
    pub const KDL_SESSION_LIFETIME: &str = "session-lifetime";

    // Sub-block names
    pub const KDL_BLOCK_STARTUP: &str = "startup";
    pub const KDL_BLOCK_QUALITY_RESOLUTION: &str = "quality-resolution";
    pub const KDL_BLOCK_CANONICALIZATION: &str = "canonicalization";
    pub const KDL_BLOCK_HEALTH_DETECTION: &str = "health-detection";
    pub const KDL_BLOCK_PERFORMANCE: &str = "performance";
    pub const KDL_BLOCK_TAG_SPLITTING: &str = "tag-splitting";
    pub const KDL_BLOCK_DUPLICATE_ANALYSIS: &str = "duplicate-analysis";
    pub const KDL_BLOCK_RELEASE_PACKING: &str = "release-packing";
    pub const KDL_BLOCK_INBOX_ORGANIZE: &str = "inbox-organize";
    pub const KDL_BLOCK_EXTERNAL_MATCHING: &str = "external-matching";
    pub const KDL_BLOCK_DISC_EXTRACTION: &str = "disc-extraction";
    pub const KDL_BLOCK_ALBUM_ART: &str = "album-art";
}

/// Opinions for quality-based auto-resolution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityResolutionOpinions {
    /// Inbox-to-corpus bitrate fuzz tolerance as a percentage (default: 5.0).
    /// Files within this % bitrate difference (same format class and sample rate)
    /// are treated as equivalent rather than superior/inferior. Suppresses noise
    /// from minor FLAC compression differences across encoder versions.
    pub inbox_bitrate_fuzz_percent: f64,
}

impl Default for QualityResolutionOpinions {
    fn default() -> Self {
        Self {
            inbox_bitrate_fuzz_percent: 5.0,
        }
    }
}

impl QualityResolutionOpinions {
    pub const KDL_BITRATE_FUZZ: &str = "inbox-bitrate-fuzz-percent";
}

/// Opinions for tag canonicalization
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CanonicalizationOpinions {
    /// Strip EP/LP suffixes during album collision detection (default: false).
    /// When true, "Album EP" and "Album" normalize to the same key and collide.
    pub strip_album_format_suffixes: bool,
}

impl CanonicalizationOpinions {
    pub const KDL_STRIP_SUFFIXES: &str = "strip-album-format-suffixes";
}

/// Which view to land on after startup progress completes.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
pub enum StartupView {
    #[default]
    Health,
    Search,
    Browser,
    Inbox,
    ExternalMatches,
}

/// Opinions for startup behavior
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartupOpinions {
    /// Force verification of all indexed files at startup, bypassing mtime optimization (default: false).
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

impl StartupOpinions {
    pub const KDL_FORCE_CHECK: &str = "force-check-all-files";
    pub const KDL_VACUUM_THRESHOLD: &str = "vacuum-threshold";
    pub const KDL_DEFAULT_VIEW: &str = "default-view";
}

/// Opinions for health detection behavior
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthDetectionOpinions {
    /// Tags that must be present on every track (default: title, album, artist, album_artist)
    pub required_tags: Vec<String>,
    /// When true, album_artist is only required on compilation albums (>1 distinct artist).
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

impl HealthDetectionOpinions {
    pub const KDL_REQUIRED_TAGS: &str = "required-tags";
    pub const KDL_ALBUM_ARTIST_COMPILATION: &str = "album-artist-only-required-if-compilation";
    pub const KDL_SINGLE_ALBUM_SUFFIX: &str = "single-album-suffix";
}

/// Opinions for performance tuning (threads, caches)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceOpinions {
    /// Number of worker threads. None = 2x logical cores (default).
    pub worker_threads: Option<usize>,
    /// SQLite page cache size per connection in MB (default: 256).
    pub db_cache_mb: u32,
}

impl Default for PerformanceOpinions {
    fn default() -> Self {
        Self {
            worker_threads: None,
            db_cache_mb: 256,
        }
    }
}

impl PerformanceOpinions {
    pub const KDL_WORKER_THREADS: &str = "worker-threads";
    pub const KDL_DB_CACHE: &str = "db-cache";
}

/// Opinions for detecting and splitting compound tag values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TagSplittingOpinions {
    /// Collaboration keywords for artist tags (e.g., "feat", "ft", "featuring", "vs", "with").
    pub collaboration_keywords: std::collections::HashSet<String>,
    /// Per-tag separator strings. Key is uppercase tag name (e.g., "ARTIST", "GENRE").
    pub tag_separators: std::collections::HashMap<String, Vec<String>>,
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

        Self {
            collaboration_keywords,
            tag_separators,
        }
    }
}

impl TagSplittingOpinions {
    pub const KDL_COLLAB: &str = "collab";
}

/// Opinions for fingerprint duplicate analysis.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DuplicateAnalysisOpinions {
    /// Fingerprint similarity threshold (0-100). Default: 95.0
    pub fingerprint_similarity_threshold: f64,
    /// Duration tolerance in milliseconds. Default: 2000 (2 seconds)
    pub duration_tolerance_ms: i64,
    /// Skip duplicate pairs where either title contains variant keywords. Default: true
    pub elide_variant_titles: bool,
}

impl Default for DuplicateAnalysisOpinions {
    fn default() -> Self {
        Self {
            fingerprint_similarity_threshold: 95.0,
            duration_tolerance_ms: 2000,
            elide_variant_titles: true,
        }
    }
}

impl DuplicateAnalysisOpinions {
    pub const KDL_FP_THRESHOLD: &str = "fingerprint-similarity-threshold";
    pub const KDL_DURATION_TOLERANCE: &str = "duration-tolerance-ms";
    pub const KDL_ELIDE_VARIANTS: &str = "elide-variant-titles";
}

/// Scoring dimension weights for release bin-packing assignment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackingWeights {
    pub acoustid_confidence: f64,
    pub duration_match: f64,
    pub title_match: f64,
    pub artist_match: f64,
    pub album_match: f64,
    pub track_number_match: f64,
}

impl PackingWeights {
    /// Default weights for AcoustID-backed candidate scoring.
    pub fn candidate_defaults() -> Self {
        Self {
            acoustid_confidence: 0.30,
            duration_match: 0.30,
            title_match: 0.10,
            artist_match: 0.05,
            album_match: 0.05,
            track_number_match: 0.20,
        }
    }

    /// Default weights for elimination/gap-filling scoring.
    pub fn elimination_defaults() -> Self {
        Self {
            acoustid_confidence: 0.0,
            duration_match: 0.25,
            title_match: 0.30,
            artist_match: 0.0,
            album_match: 0.05,
            track_number_match: 0.40,
        }
    }

    pub const KDL_ACOUSTID_CONFIDENCE: &str = "acoustid-confidence";
    pub const KDL_DURATION_MATCH: &str = "duration-match";
    pub const KDL_TITLE_MATCH: &str = "title-match";
    pub const KDL_ARTIST_MATCH: &str = "artist-match";
    pub const KDL_ALBUM_MATCH: &str = "album-match";
    pub const KDL_TRACK_NUMBER_MATCH: &str = "track-number-match";
}

/// Opinions for MusicBrainz release bin-packing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReleasePackingOpinions {
    pub duration_tolerance_pct: f64,
    pub min_confidence: f64,
    pub candidate_weights: PackingWeights,
    pub elimination_weights: PackingWeights,
    pub title_preassign_threshold: f64,
    pub packing_knot_ratio: f64,
    pub packing_knot_size_limit: usize,
    pub singles_before_incompletes: bool,
    pub allow_resolve_knots_with_discographies: bool,
    pub low_confidence_max_acoustid_ratio: f64,
    pub low_confidence_max_album_match: f64,
}

impl Default for ReleasePackingOpinions {
    fn default() -> Self {
        Self {
            duration_tolerance_pct: 0.15,
            min_confidence: 0.3,
            candidate_weights: PackingWeights::candidate_defaults(),
            elimination_weights: PackingWeights::elimination_defaults(),
            title_preassign_threshold: Self::DEFAULT_TITLE_PREASSIGN_THRESHOLD,
            packing_knot_ratio: Self::DEFAULT_PACKING_KNOT_RATIO,
            packing_knot_size_limit: Self::DEFAULT_PACKING_KNOT_SIZE_LIMIT,
            singles_before_incompletes: true,
            allow_resolve_knots_with_discographies: true,
            low_confidence_max_acoustid_ratio: Self::DEFAULT_LOW_CONFIDENCE_MAX_ACOUSTID_RATIO,
            low_confidence_max_album_match: Self::DEFAULT_LOW_CONFIDENCE_MAX_ALBUM_MATCH,
        }
    }
}

impl ReleasePackingOpinions {
    pub const DEFAULT_TITLE_PREASSIGN_THRESHOLD: f64 = 0.95;
    pub const DEFAULT_PACKING_KNOT_RATIO: f64 = 3.0;
    pub const DEFAULT_PACKING_KNOT_SIZE_LIMIT: usize = 50;
    pub const DEFAULT_LOW_CONFIDENCE_MAX_ACOUSTID_RATIO: f64 = 0.25;
    pub const DEFAULT_LOW_CONFIDENCE_MAX_ALBUM_MATCH: f64 = 0.30;

    pub const KDL_DURATION_TOLERANCE_PCT: &str = "duration-tolerance-pct";
    pub const KDL_MIN_CONFIDENCE: &str = "min-confidence";
    pub const KDL_CANDIDATE_WEIGHTS: &str = "candidate-weights";
    pub const KDL_ELIMINATION_WEIGHTS: &str = "elimination-weights";
    pub const KDL_TITLE_PREASSIGN_THRESHOLD: &str = "title-preassign-threshold";
    pub const KDL_PACKING_KNOT_RATIO: &str = "packing-knot-ratio";
    pub const KDL_PACKING_KNOT_SIZE_LIMIT: &str = "packing-knot-size-limit";
    pub const KDL_SINGLES_BEFORE_INCOMPLETES: &str = "singles-before-incompletes";
    pub const KDL_ALLOW_DISCOGRAPHY_REDUCTION: &str = "allow-resolve-knots-with-discographies";
    pub const KDL_LOW_CONFIDENCE_ACOUSTID_RATIO: &str = "low-confidence-max-acoustid-ratio";
    pub const KDL_LOW_CONFIDENCE_ALBUM_MATCH: &str = "low-confidence-max-album-match";
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
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct InboxOrganizeOpinions {
    pub directory_granularity: InboxOrganizeGranularity,
}

impl InboxOrganizeOpinions {
    pub const KDL_DIR_GRANULARITY: &str = "directory-granularity";
}

/// How a recording-level MusicBrainz relation type maps to tag output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelationRouting {
    pub artist: bool,
    pub title: bool,
    pub composer: bool,
}

impl RelationRouting {
    pub const SKIP: Self = Self {
        artist: false,
        title: false,
        composer: false,
    };
}

/// Per-relation-type routing rules for recording credits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreditRoutingConfig {
    pub routing: std::collections::HashMap<String, RelationRouting>,
    /// Format string for vocalist title suffix. Default: "feat. {artists}".
    pub feat_format: String,
}

impl Default for CreditRoutingConfig {
    fn default() -> Self {
        let mut routing = std::collections::HashMap::new();
        routing.insert(
            "performer".to_string(),
            RelationRouting {
                artist: true,
                title: false,
                composer: false,
            },
        );
        routing.insert(
            "vocal".to_string(),
            RelationRouting {
                artist: true,
                title: true,
                composer: false,
            },
        );
        routing.insert(
            "instrument".to_string(),
            RelationRouting {
                artist: true,
                title: false,
                composer: false,
            },
        );
        routing.insert(
            "remixer".to_string(),
            RelationRouting {
                artist: false,
                title: true,
                composer: false,
            },
        );
        Self {
            routing,
            feat_format: "feat. {artists}".to_string(),
        }
    }
}

impl CreditRoutingConfig {
    pub const KDL_CREDIT_ROUTING: &str = "credit-routing";
    pub const KDL_FEAT_FORMAT: &str = "feat-format";

    /// Look up routing for a relation type, defaulting to SKIP.
    pub fn route_for(&self, relation_type: &str) -> &RelationRouting {
        self.routing
            .get(relation_type)
            .unwrap_or(&RelationRouting::SKIP)
    }
}

/// Configuration for external metadata matching (AcoustID, MusicBrainz).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalMatchingConfig {
    pub acoustid_api_key: String,
    pub requests_per_second: u32,
    pub mb_requests_per_second: u32,
    pub mb_base_url: String,
    pub auto_enrich_on_match: bool,
    pub mb_cache_ttl_days: u32,
    pub preferred_locales: Vec<String>,
    pub tag_templates: Vec<(String, String)>,
    pub credit_routing: CreditRoutingConfig,
}

impl Default for ExternalMatchingConfig {
    fn default() -> Self {
        Self {
            acoustid_api_key: String::new(),
            requests_per_second: 3,
            mb_requests_per_second: 25,
            mb_base_url: Self::DEFAULT_MB_BASE_URL.to_string(),
            auto_enrich_on_match: true,
            mb_cache_ttl_days: 30,
            preferred_locales: Vec::new(),
            tag_templates: Vec::new(),
            credit_routing: CreditRoutingConfig::default(),
        }
    }
}

impl ExternalMatchingConfig {
    pub const DEFAULT_MB_BASE_URL: &str = "https://musicbrainz.org/ws/2";

    pub const KDL_ACOUSTID_KEY: &str = "acoustid-api-key";
    pub const KDL_REQ_PER_SEC: &str = "requests-per-second";
    pub const KDL_MB_REQ_PER_SEC: &str = "mb-requests-per-second";
    pub const KDL_MB_BASE_URL: &str = "mb-base-url";
    pub const KDL_AUTO_ENRICH: &str = "auto-enrich-on-match";
    pub const KDL_MB_CACHE_TTL: &str = "mb-cache-ttl-days";
    pub const KDL_PREFERRED_LOCALES: &str = "preferred-locales";
    pub const KDL_TAG_TEMPLATES: &str = "tag-templates";
    pub const KDL_CREDIT_ROUTING: &str = "credit-routing";
}

/// Opinions for disc extraction from ALBUM and TRACKNUMBER tags.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiscExtractionOpinions {
    pub disc_tag_name: String,
    pub map_letters_to_numbers: bool,
}

impl Default for DiscExtractionOpinions {
    fn default() -> Self {
        Self {
            disc_tag_name: "DISCNUMBER".to_string(),
            map_letters_to_numbers: false,
        }
    }
}

impl DiscExtractionOpinions {
    pub const KDL_DISC_TAG_NAME: &str = "disc-tag-name";
    pub const KDL_MAP_LETTERS: &str = "map-letters-to-numbers";
}

/// Controls whether sidecar images are deployed alongside audio files.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
pub enum SidecarDeployMode {
    Disabled,
    #[default]
    PrimaryCover,
    All,
}

/// Album art embedding and upgrade configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AlbumArtOpinions {
    pub sidecar_deploy_mode: SidecarDeployMode,
}

impl AlbumArtOpinions {
    pub const KDL_SIDECAR_DEPLOY: &str = "sidecar-deploy-mode";
}

/// Debug and diagnostic options (reserved for future use).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DebugOpinions {}

impl Default for Opinions {
    fn default() -> Self {
        Self {
            quality_resolution: QualityResolutionOpinions::default(),
            canonicalization: CanonicalizationOpinions::default(),
            startup: StartupOpinions::default(),
            health_detection: HealthDetectionOpinions::default(),
            performance: PerformanceOpinions::default(),
            tag_splitting: TagSplittingOpinions::default(),
            duplicate_analysis: DuplicateAnalysisOpinions::default(),
            release_packing: ReleasePackingOpinions::default(),
            inbox_organize: InboxOrganizeOpinions::default(),
            leave_transactions_open: false,
            external_matching: ExternalMatchingConfig::default(),
            disc_extraction: DiscExtractionOpinions::default(),
            album_art: AlbumArtOpinions::default(),
            debug: DebugOpinions::default(),
            watcher_poll_interval_secs: 900,
            session_lifetime_days: Some(30),
        }
    }
}

/// A configured source directory within the corpus.
///
/// Boolean fields are `Option<bool>` — `None` means "inherit from parent
/// source directory" (or fall back to system default if no parent sets it).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceDir {
    pub path: PathBuf,
    pub libraries: Vec<String>,
    pub can_stash_dupes: Option<bool>,
    pub interior_dupes: Option<bool>,
    pub path_schema: Option<PathTagSchema>,
    pub enable_acoustid: Option<bool>,
    pub pinned_release: Option<String>,
}

impl SourceDir {
    /// Whether this source dir has all-default/inherit settings.
    pub fn is_default(&self) -> bool {
        self.libraries.is_empty()
            && self.can_stash_dupes.is_none()
            && self.interior_dupes.is_none()
            && self.path_schema.is_none()
            && self.enable_acoustid.is_none()
            && self.pinned_release.is_none()
    }
}

/// Fully resolved source configuration for a specific path.
///
/// All fields have concrete values — inheritance has been flattened.
#[derive(Debug, Clone)]
pub struct ResolvedSourceConfig {
    pub source_path: PathBuf,
    pub libraries: Vec<String>,
    pub can_stash_dupes: bool,
    pub interior_dupes: bool,
    pub path_schema: Option<PathTagSchema>,
    pub enable_acoustid: bool,
}

/// Shared config wrapped in `Arc<RwLock<Config>>` for thread-safe read/write access.
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

    pub fn corpus_dir(&self) -> PathBuf {
        self.root.join("corpus")
    }

    pub fn libraries_dir(&self) -> PathBuf {
        self.root.join("libraries")
    }

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
    pub fn get_corpus_paths_for_library(&self, library_name: &str) -> Vec<PathBuf> {
        let corpus_dir = self.corpus_dir();
        self.source_dirs
            .iter()
            .filter(|sd| sd.libraries.contains(&library_name.to_string()))
            .map(|sd| corpus_dir.join(&sd.path))
            .collect()
    }

    /// Check if a zone-relative file path is under a configured source directory.
    pub fn is_path_in_source(&self, path: &Path) -> bool {
        self.source_dirs.iter().any(|sd| path.starts_with(&sd.path))
    }

    /// Resolve the full source config for a corpus-relative path, with inheritance.
    ///
    /// Takes a path relative to corpus root (WITHOUT "corpus/" prefix).
    /// Returns None if not under any configured source directory.
    pub fn resolve_source_config(&self, relative_path: &Path) -> Option<ResolvedSourceConfig> {
        let mut matching: Vec<&SourceDir> = self
            .source_dirs
            .iter()
            .filter(|sd| relative_path.starts_with(&sd.path))
            .collect();

        if matching.is_empty() {
            return None;
        }

        matching.sort_by(|a, b| b.path.as_os_str().len().cmp(&a.path.as_os_str().len()));

        let source_path = matching[0].path.clone();

        let libraries = matching
            .iter()
            .find(|sd| !sd.libraries.is_empty())
            .map(|sd| sd.libraries.clone())
            .unwrap_or_default();

        let can_stash_dupes = matching
            .iter()
            .find_map(|sd| sd.can_stash_dupes)
            .unwrap_or(true);

        let interior_dupes = matching
            .iter()
            .find_map(|sd| sd.interior_dupes)
            .unwrap_or(true);

        let path_schema = matching.iter().find_map(|sd| sd.path_schema.clone());

        let enable_acoustid = matching
            .iter()
            .find_map(|sd| sd.enable_acoustid)
            .unwrap_or(true);

        Some(ResolvedSourceConfig {
            source_path,
            libraries,
            can_stash_dupes,
            interior_dupes,
            path_schema,
            enable_acoustid,
        })
    }

    /// Resolve source config for a DB path (zone-relative, no prefix).
    pub fn resolve_source_config_for_db_path(
        &self,
        db_path: &str,
    ) -> Option<ResolvedSourceConfig> {
        self.resolve_source_config(Path::new(db_path))
    }

    /// Get the raw SourceDir for an exact path match (for config editing).
    pub fn get_raw_source_dir(&self, relative_path: &Path) -> Option<&SourceDir> {
        self.source_dirs.iter().find(|sd| sd.path == relative_path)
    }

    /// Clamp root source dir `None` values to system defaults.
    ///
    /// `None` means "inherit from parent" — the root dir has no parent, so
    /// `None` on root is a logic error. This replaces `None` with the concrete
    /// system defaults so all downstream code sees resolved values.
    pub fn clamp_root_defaults(&mut self) {
        if let Some(root) = self.source_dirs.iter_mut().find(|sd| sd.path == Path::new("")) {
            if root.can_stash_dupes.is_none() {
                root.can_stash_dupes = Some(true);
            }
            if root.interior_dupes.is_none() {
                root.interior_dupes = Some(true);
            }
            if root.enable_acoustid.is_none() {
                root.enable_acoustid = Some(true);
            }
        }
    }

    /// Compute DB-path prefixes for source dirs where AcoustID is disabled.
    ///
    /// Returns zone-relative prefixes (no "corpus/" prefix).
    /// Returns empty vec when no dirs are excluded (the common case).
    /// Used by the fetch scheduler to build SQL `NOT LIKE` exclusion clauses.
    pub fn acoustid_excluded_db_prefixes(&self) -> Vec<String> {
        self.source_dirs
            .iter()
            .filter(|sd| {
                let resolved = self.resolve_source_config(&sd.path);
                resolved.is_some_and(|r| !r.enable_acoustid)
            })
            .map(|sd| sd.path.display().to_string())
            .collect()
    }
}
