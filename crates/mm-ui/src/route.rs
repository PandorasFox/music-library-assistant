//! URL-driven route types for MM's UI.
//!
//! Every representable page is a [`Route`] variant carrying URL-encodable
//! parameters. Both the WASM client (URL ↔ Route) and TUI (ActiveView ↔ Route)
//! use this as the shared "what page am I on" type.
//!
//! Path segments carry identity (which view, which entity).
//! Query parameters carry position (cursor, scroll, focus, filter text).

use mm_meta::db_types::Zone;
use mm_meta::views::ConflictBucket;
use mm_meta::views::external_matches::{AcoustidConfidence, ReleaseReviewFilter};
use std::fmt;

// ============================================================================
// Route
// ============================================================================

/// Every representable page in MM's UI, with all URL-encodable parameters.
#[derive(Debug, Clone, PartialEq)]
pub enum Route {
    // Lateral views
    Config(ConfigRoute),
    Search(SearchRoute),
    Files(FilesRoute),
    Health(HealthRoute),
    History(HistoryRoute),
    Transaction(TransactionRoute),
    Deploy(DeployRoute),
    ExternalMatches(ExternalMatchesRoute),

    // Overlay views (pushed on top of lateral context)
    TagEditor(TagEditorRoute),
    Resolution(ResolutionRoute),
    TransactionReview(TransactionReviewRoute),
    PackingBrowser(PackingBrowserRoute),
    KnotBrowser(KnotBrowserRoute),
    ExternalMatchOverlay(ExternalMatchRoute),
}

// ============================================================================
// Lateral view routes
// ============================================================================

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ConfigRoute {
    pub cursor: Option<usize>,
    /// Field name currently being edited.
    pub editing: Option<String>,
    pub focus: Option<FocusTarget>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SearchRoute {
    /// builder | results
    pub mode: Option<SearchMode>,
    /// Compact query encoding (conditions serialized to string).
    pub query: Option<String>,
    /// Cursor in the result list.
    pub cursor: Option<usize>,
    pub focused_condition: Option<usize>,
    pub field_focus: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct FilesRoute {
    /// Currently selected directory path (corpus-relative).
    pub path: Option<String>,
    /// Search bar text.
    pub search: Option<String>,
    /// "config" if the directory config panel is open.
    pub panel: Option<String>,
    /// Cursor within the config panel.
    pub panel_field: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct HealthRoute {
    pub cursor: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct HistoryRoute {
    /// Expanded session ID.
    pub session: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct TransactionRoute {
    pub cursor: Option<usize>,
    pub focus: Option<FocusTarget>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DeployRoute {
    pub tab: Option<crate::domain_types::DeployTab>,
    pub scroll: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ExternalMatchesRoute {
    pub cursor: Option<usize>,
}

// ============================================================================
// Overlay view routes
// ============================================================================

#[derive(Debug, Clone, PartialEq)]
pub struct TagEditorRoute {
    /// Inode(s) to edit. Single-element for single file, multiple for bulk.
    pub inodes: Vec<i64>,
    /// individual | aggregate (bulk only).
    pub mode: Option<String>,
    /// Current track index (bulk).
    pub item: Option<usize>,
    /// Current field index.
    pub field: Option<usize>,
    /// "name" | "value" — which part of the field is being edited.
    pub editing: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct TransactionReviewRoute {
    pub cursor: Option<usize>,
    pub focus: Option<FocusTarget>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PackingBrowserRoute {
    pub category: String,
    pub cursor: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct KnotBrowserRoute {
    pub id: String,
    pub cursor: Option<usize>,
}

// ============================================================================
// External match overlay routes
// ============================================================================

/// Sub-routes under `/external-matches/` for AcoustID browse and release review.
/// These are overlay views launched from the external matches lateral view.
#[derive(Debug, Clone, PartialEq)]
pub enum ExternalMatchRoute {
    /// Browse AcoustID matches filtered by confidence tier.
    AcoustidBrowse { confidence: AcoustidConfidence, cursor: Option<usize> },
    /// Browse packed releases filtered by review category.
    ReleaseReview { filter: ReleaseReviewFilter, cursor: Option<usize> },
    /// Single release detail view for approval.
    ReleaseDetail { release_id: String, cursor: Option<usize> },
}

// ============================================================================
// Resolution routes
// ============================================================================

/// Discriminant for resolution modal type + keying parameters.
#[derive(Debug, Clone, PartialEq)]
pub enum ResolutionRoute {
    // === Simple-batch (path alone = load anchor) ===
    MissingFilesRestorable { cursor: Option<usize>, focus: Option<FocusTarget> },
    MissingFilesPermanent { cursor: Option<usize> },
    MissingDirectories { cursor: Option<usize> },
    CorruptFiles { cursor: Option<usize> },
    LosslessRemux { cursor: Option<usize> },
    SubparDuplicates { cursor: Option<usize> },
    DirectoryCluster { cluster: Option<usize>, cursor: Option<usize> },
    MovedFiles { cursor: Option<usize> },
    OobResolution { bucket: Option<mm_meta::views::ConflictBucket>, cursor: Option<usize> },

    // === Cluster-nav (path + identity params = load anchor) ===
    TagCanonicity { tag_name: String, zone: Zone, cluster: Option<usize> },
    InconsistentAlbumArtist { tag_name: String, cluster: Option<usize> },
    CompoundSplit { tag_name: String, zone: Zone, safe_mode: bool, cluster: Option<usize> },
    MissingAlbum { group: Option<usize> },
    DiscExtraction { group: Option<usize> },

    // === Group-review (path + group = load anchor) ===
    RedundantDuplicates { group: Option<usize> },
    DeployConflicts { group: Option<usize> },
    MetadataDuplicates { group: Option<usize> },
    SameRecording { group: Option<usize> },
}

impl ResolutionRoute {
    /// Canonical human-readable label for this resolution type.
    pub fn display_label(&self) -> &'static str {
        match self {
            Self::MissingFilesRestorable { .. } => "Missing Files (Restorable)",
            Self::MissingFilesPermanent { .. } => "Missing Files (Permanent)",
            Self::MissingDirectories { .. } => "Missing Directories",
            Self::CorruptFiles { .. } => "Corrupt Files",
            Self::LosslessRemux { .. } => "Lossless Remux",
            Self::SubparDuplicates { .. } => "Subpar Duplicates",
            Self::DirectoryCluster { .. } => "Directory Cluster",
            Self::MovedFiles { .. } => "Moved Files",
            Self::OobResolution { .. } => "OOB Resolution",
            Self::TagCanonicity { .. } => "Tag Canonicity",
            Self::InconsistentAlbumArtist { .. } => "Inconsistent Album Artist",
            Self::CompoundSplit { .. } => "Compound Split",
            Self::MissingAlbum { .. } => "Missing Album",
            Self::DiscExtraction { .. } => "Disc Extraction",
            Self::RedundantDuplicates { .. } => "Redundant Duplicates",
            Self::DeployConflicts { .. } => "Deploy Conflicts",
            Self::MetadataDuplicates { .. } => "Metadata Duplicates",
            Self::SameRecording { .. } => "Same Recording",
        }
    }
}

// ============================================================================
// Shared small types
// ============================================================================

/// Focus target for views with list + buttons layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusTarget {
    List,
    Buttons,
}

/// Search mode for the tag search view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchMode {
    Builder,
    Results,
}

// ============================================================================
// Parse error
// ============================================================================

#[derive(Debug, Clone)]
pub struct RouteParseError {
    pub message: String,
}

impl fmt::Display for RouteParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "route parse error: {}", self.message)
    }
}

// ============================================================================
// URL serialization
// ============================================================================

impl Route {
    /// Serialize this route to a URL path + query string.
    pub fn to_url(&self) -> String {
        let mut path = String::new();
        let mut params = QueryParams::new();

        match self {
            Route::Config(r) => {
                path.push_str("/config");
                params.set_usize("cursor", r.cursor);
                params.set_str("editing", r.editing.as_deref());
                params.set_focus("focus", r.focus);
            }
            Route::Search(r) => {
                path.push_str("/search");
                params.set_str("mode", r.mode.map(|m| match m {
                    SearchMode::Builder => "builder",
                    SearchMode::Results => "results",
                }));
                params.set_str("q", r.query.as_deref());
                params.set_usize("cursor", r.cursor);
                params.set_usize("condition", r.focused_condition);
                params.set_str("field", r.field_focus.as_deref());
            }
            Route::Files(r) => {
                if let Some(ref p) = r.path {
                    path.push_str("/files/");
                    path.push_str(p);
                } else {
                    path.push_str("/files");
                }
                params.set_str("search", r.search.as_deref());
                params.set_str("panel", r.panel.as_deref());
                params.set_usize("panel_field", r.panel_field);
            }
            Route::Health(r) => {
                path.push_str("/health");
                params.set_usize("cursor", r.cursor);
            }
            Route::History(r) => {
                path.push_str("/history");
                if let Some(sid) = r.session {
                    params.set_str("session", Some(&sid.to_string()));
                }
            }
            Route::Transaction(r) => {
                path.push_str("/transaction");
                params.set_usize("cursor", r.cursor);
                params.set_focus("focus", r.focus);
            }
            Route::Deploy(r) => {
                path.push_str("/deploy");
                params.set_str("tab", r.tab.map(|t| t.as_str()));
                params.set_usize("scroll", r.scroll);
            }
            Route::ExternalMatches(r) => {
                path.push_str("/external-matches");
                params.set_usize("cursor", r.cursor);
            }
            Route::TagEditor(r) => {
                if r.inodes.len() == 1 {
                    path.push_str(&format!("/tags/{}", r.inodes[0]));
                } else {
                    path.push_str("/tags");
                    let csv: Vec<String> = r.inodes.iter().map(|i| i.to_string()).collect();
                    params.set_str("inodes", Some(&csv.join(",")));
                }
                params.set_str("mode", r.mode.as_deref());
                params.set_usize("item", r.item);
                params.set_usize("field", r.field);
                params.set_str("editing", r.editing.as_deref());
            }
            Route::Resolution(r) => {
                resolution_to_url(r, &mut path, &mut params);
            }
            Route::TransactionReview(r) => {
                path.push_str("/review");
                params.set_usize("cursor", r.cursor);
                params.set_focus("focus", r.focus);
            }
            Route::PackingBrowser(r) => {
                path.push_str("/packing/");
                path.push_str(&r.category);
                params.set_usize("cursor", r.cursor);
            }
            Route::KnotBrowser(r) => {
                path.push_str("/knots/");
                path.push_str(&r.id);
                params.set_usize("cursor", r.cursor);
            }
            Route::ExternalMatchOverlay(r) => {
                external_match_overlay_to_url(r, &mut path, &mut params);
            }
        }

        let query = params.to_query_string();
        if query.is_empty() {
            path
        } else {
            format!("{path}?{query}")
        }
    }

    /// Parse a route from a URL path and query parameter pairs.
    pub fn from_url(path: &str, query: &[(String, String)]) -> Result<Route, RouteParseError> {
        let params = QueryParams::from_pairs(query);
        let path = path.trim_start_matches('/');

        // Strip leading hash if present (for hash-based routing).
        let path = path.trim_start_matches('#');

        // Split path into segments.
        let segments: Vec<&str> = if path.is_empty() {
            vec![]
        } else {
            path.split('/').collect()
        };

        match segments.first().copied().unwrap_or("health") {
            "config" => Ok(Route::Config(ConfigRoute {
                cursor: params.get_usize("cursor"),
                editing: params.get_string("editing"),
                focus: params.get_focus("focus"),
            })),
            "search" => Ok(Route::Search(SearchRoute {
                mode: params.get_string("mode").and_then(|s| match s.as_str() {
                    "builder" => Some(SearchMode::Builder),
                    "results" => Some(SearchMode::Results),
                    _ => None,
                }),
                query: params.get_string("q"),
                cursor: params.get_usize("cursor"),
                focused_condition: params.get_usize("condition"),
                field_focus: params.get_string("field"),
            })),
            "files" => {
                let file_path = if segments.len() > 1 {
                    Some(segments[1..].join("/"))
                } else {
                    None
                };
                Ok(Route::Files(FilesRoute {
                    path: file_path,
                    search: params.get_string("search"),
                    panel: params.get_string("panel"),
                    panel_field: params.get_usize("panel_field"),
                }))
            }
            "health" => Ok(Route::Health(HealthRoute {
                cursor: params.get_usize("cursor"),
            })),
            "history" => Ok(Route::History(HistoryRoute {
                session: params.get_string("session").and_then(|s| s.parse().ok()),
            })),
            "transaction" => Ok(Route::Transaction(TransactionRoute {
                cursor: params.get_usize("cursor"),
                focus: params.get_focus("focus"),
            })),
            "deploy" => Ok(Route::Deploy(DeployRoute {
                tab: params.get_string("tab").and_then(|s| crate::domain_types::DeployTab::from_str(&s)),
                scroll: params.get_usize("scroll"),
            })),
            "external-matches" => {
                // Sub-routes: /external-matches/acoustid/{confidence}
                //             /external-matches/review/{filter}
                //             /external-matches/review/release/{id}
                if let Some(sub) = segments.get(1) {
                    match *sub {
                        "acoustid" => {
                            let confidence_str = segments.get(2).ok_or_else(|| RouteParseError {
                                message: "acoustid browse requires a confidence tier".into(),
                            })?;
                            let confidence = AcoustidConfidence::from_str(confidence_str)
                                .ok_or_else(|| RouteParseError {
                                    message: format!("unknown confidence tier: {confidence_str}"),
                                })?;
                            Ok(Route::ExternalMatchOverlay(ExternalMatchRoute::AcoustidBrowse {
                                confidence,
                                cursor: params.get_usize("cursor"),
                            }))
                        }
                        "review" => {
                            // /external-matches/review/release/{id} OR /external-matches/review/{filter}
                            let next = segments.get(2).ok_or_else(|| RouteParseError {
                                message: "review requires a filter or 'release'".into(),
                            })?;
                            if *next == "release" {
                                let release_id = segments.get(3).ok_or_else(|| RouteParseError {
                                    message: "release detail requires a release_id".into(),
                                })?;
                                Ok(Route::ExternalMatchOverlay(ExternalMatchRoute::ReleaseDetail {
                                    release_id: (*release_id).to_string(),
                                    cursor: params.get_usize("cursor"),
                                }))
                            } else {
                                let filter = ReleaseReviewFilter::from_str(next)
                                    .ok_or_else(|| RouteParseError {
                                        message: format!("unknown release review filter: {next}"),
                                    })?;
                                Ok(Route::ExternalMatchOverlay(ExternalMatchRoute::ReleaseReview {
                                    filter,
                                    cursor: params.get_usize("cursor"),
                                }))
                            }
                        }
                        _ => Err(RouteParseError {
                            message: format!("unknown external-matches sub-route: {sub}"),
                        }),
                    }
                } else {
                    Ok(Route::ExternalMatches(ExternalMatchesRoute {
                        cursor: params.get_usize("cursor"),
                    }))
                }
            }
            "tags" => {
                let (inodes, mode) = if segments.len() > 1 {
                    // /tags/{inode} — single file
                    let inode: i64 = segments[1].parse().map_err(|_| RouteParseError {
                        message: format!("invalid inode: {}", segments[1]),
                    })?;
                    (vec![inode], None)
                } else {
                    // /tags?inodes=1,2,3 — bulk edit
                    let csv = params.get_string("inodes").unwrap_or_default();
                    let inodes: Vec<i64> = csv
                        .split(',')
                        .filter(|s| !s.is_empty())
                        .filter_map(|s| s.parse().ok())
                        .collect();
                    if inodes.is_empty() {
                        return Err(RouteParseError {
                            message: "tag editor requires at least one inode".into(),
                        });
                    }
                    (inodes, params.get_string("mode"))
                };
                Ok(Route::TagEditor(TagEditorRoute {
                    inodes,
                    mode,
                    item: params.get_usize("item"),
                    field: params.get_usize("field"),
                    editing: params.get_string("editing"),
                }))
            }
            "resolve" => resolution_from_url(&segments, &params),
            "review" => Ok(Route::TransactionReview(TransactionReviewRoute {
                cursor: params.get_usize("cursor"),
                focus: params.get_focus("focus"),
            })),
            "packing" => {
                let category = segments.get(1).ok_or_else(|| RouteParseError {
                    message: "packing browser requires a category".into(),
                })?;
                Ok(Route::PackingBrowser(PackingBrowserRoute {
                    category: (*category).to_string(),
                    cursor: params.get_usize("cursor"),
                }))
            }
            "knots" => {
                let id = segments.get(1).ok_or_else(|| RouteParseError {
                    message: "knot browser requires an id".into(),
                })?;
                Ok(Route::KnotBrowser(KnotBrowserRoute {
                    id: (*id).to_string(),
                    cursor: params.get_usize("cursor"),
                }))
            }
            other => Err(RouteParseError {
                message: format!("unknown route: {other}"),
            }),
        }
    }

    /// Map this route to its [`LateralView`], if it's a lateral view.
    pub fn lateral_view(&self) -> Option<crate::lateral_view::LateralView> {
        use crate::lateral_view::LateralView;
        match self {
            Route::Config(_) => Some(LateralView::Config),
            Route::Search(_) => Some(LateralView::Search),
            Route::Files(_) => Some(LateralView::Files),
            Route::Health(_) => Some(LateralView::Health),
            Route::History(_) => Some(LateralView::History),
            Route::Transaction(_) => Some(LateralView::Transaction),
            Route::Deploy(_) => Some(LateralView::Deploy),
            Route::ExternalMatches(_) => Some(LateralView::ExternalMatches),
            _ => None,
        }
    }

    /// The lateral view to highlight in the titlebar for any route.
    ///
    /// For lateral views, returns that view directly.
    /// For overlays, returns the parent lateral view they're associated with.
    pub fn parent_lateral(&self) -> crate::lateral_view::LateralView {
        use crate::lateral_view::LateralView;
        self.lateral_view().unwrap_or_else(|| match self {
            Route::PackingBrowser(_) | Route::KnotBrowser(_) => LateralView::ExternalMatches,
            Route::ExternalMatchOverlay(_) => LateralView::ExternalMatches,
            Route::TagEditor(_) => LateralView::Search,
            Route::Resolution(_) => LateralView::Health,
            Route::TransactionReview(_) => LateralView::Transaction,
            // Lateral views already handled by lateral_view() above.
            _ => LateralView::Health,
        })
    }
}

// ============================================================================
// Resolution URL helpers
// ============================================================================

fn resolution_to_url(r: &ResolutionRoute, path: &mut String, params: &mut QueryParams) {
    path.push_str("/resolve/");
    match r {
        // === Simple-batch ===
        ResolutionRoute::MissingFilesRestorable { cursor, focus } => {
            path.push_str("missing-files/restorable");
            params.set_usize("cursor", *cursor);
            params.set_focus("focus", *focus);
        }
        ResolutionRoute::MissingFilesPermanent { cursor } => {
            path.push_str("missing-files/permanent");
            params.set_usize("cursor", *cursor);
        }
        ResolutionRoute::MissingDirectories { cursor } => {
            path.push_str("missing-directories");
            params.set_usize("cursor", *cursor);
        }
        ResolutionRoute::CorruptFiles { cursor } => {
            path.push_str("corrupt-files");
            params.set_usize("cursor", *cursor);
        }
        ResolutionRoute::LosslessRemux { cursor } => {
            path.push_str("lossless-remux");
            params.set_usize("cursor", *cursor);
        }
        ResolutionRoute::SubparDuplicates { cursor } => {
            path.push_str("subpar-duplicates");
            params.set_usize("cursor", *cursor);
        }
        ResolutionRoute::DirectoryCluster { cluster, cursor } => {
            path.push_str("directory-cluster");
            params.set_usize("cluster", *cluster);
            params.set_usize("cursor", *cursor);
        }
        ResolutionRoute::MovedFiles { cursor } => {
            path.push_str("moved-files");
            params.set_usize("cursor", *cursor);
        }
        ResolutionRoute::OobResolution { bucket, cursor } => {
            path.push_str("oob-resolution");
            if let Some(b) = bucket {
                params.set_str("bucket", Some(match b {
                    ConflictBucket::MtimeOnly => "mtime-only",
                    ConflictBucket::DbOnly => "db-only",
                    ConflictBucket::DiskOnly => "disk-only",
                    ConflictBucket::Conflict => "two-way",
                }));
            }
            params.set_usize("cursor", *cursor);
        }

        // === Cluster-nav ===
        ResolutionRoute::TagCanonicity { tag_name, zone, cluster } => {
            path.push_str("tag-canonicity/");
            path.push_str(tag_name);
            params.set_str("zone", Some(zone.as_str()));
            params.set_usize("cluster", *cluster);
        }
        ResolutionRoute::InconsistentAlbumArtist { tag_name, cluster } => {
            path.push_str("inconsistent-album-artist/");
            path.push_str(tag_name);
            params.set_usize("cluster", *cluster);
        }
        ResolutionRoute::CompoundSplit { tag_name, zone, safe_mode, cluster } => {
            path.push_str("compound-split/");
            path.push_str(tag_name);
            params.set_str("zone", Some(zone.as_str()));
            params.set_str("safe", Some(if *safe_mode { "true" } else { "false" }));
            params.set_usize("cluster", *cluster);
        }
        ResolutionRoute::MissingAlbum { group } => {
            path.push_str("missing-album");
            params.set_usize("group", *group);
        }
        ResolutionRoute::DiscExtraction { group } => {
            path.push_str("disc-extraction");
            params.set_usize("group", *group);
        }

        // === Group-review ===
        ResolutionRoute::RedundantDuplicates { group } => {
            path.push_str("redundant-duplicates");
            params.set_usize("group", *group);
        }
        ResolutionRoute::DeployConflicts { group } => {
            path.push_str("deploy-conflicts");
            params.set_usize("group", *group);
        }
        ResolutionRoute::MetadataDuplicates { group } => {
            path.push_str("metadata-duplicates");
            params.set_usize("group", *group);
        }
        ResolutionRoute::SameRecording { group } => {
            path.push_str("same-recording");
            params.set_usize("group", *group);
        }
    }
}

fn resolution_from_url(
    segments: &[&str],
    params: &QueryParams,
) -> Result<Route, RouteParseError> {
    let res_type = segments.get(1).ok_or_else(|| RouteParseError {
        message: "resolve requires a type".into(),
    })?;

    let route = match *res_type {
        "missing-files" => {
            let sub = segments.get(2).ok_or_else(|| RouteParseError {
                message: "missing-files requires a sub-type (restorable|permanent)".into(),
            })?;
            match *sub {
                "restorable" => ResolutionRoute::MissingFilesRestorable {
                    cursor: params.get_usize("cursor"),
                    focus: params.get_focus("focus"),
                },
                "permanent" => ResolutionRoute::MissingFilesPermanent {
                    cursor: params.get_usize("cursor"),
                },
                other => {
                    return Err(RouteParseError {
                        message: format!("unknown missing-files sub-type: {other}"),
                    });
                }
            }
        }
        "missing-directories" => ResolutionRoute::MissingDirectories {
            cursor: params.get_usize("cursor"),
        },
        "corrupt-files" => ResolutionRoute::CorruptFiles {
            cursor: params.get_usize("cursor"),
        },
        "lossless-remux" => ResolutionRoute::LosslessRemux {
            cursor: params.get_usize("cursor"),
        },
        "subpar-duplicates" => ResolutionRoute::SubparDuplicates {
            cursor: params.get_usize("cursor"),
        },
        "directory-cluster" => ResolutionRoute::DirectoryCluster {
            cluster: params.get_usize("cluster"),
            cursor: params.get_usize("cursor"),
        },
        "moved-files" => ResolutionRoute::MovedFiles {
            cursor: params.get_usize("cursor"),
        },
        "oob-resolution" => {
            let bucket = params.get_string("bucket").map(|s| match s.as_str() {
                "mtime-only" => Ok(ConflictBucket::MtimeOnly),
                "db-only" => Ok(ConflictBucket::DbOnly),
                "disk-only" => Ok(ConflictBucket::DiskOnly),
                "two-way" => Ok(ConflictBucket::Conflict),
                other => Err(RouteParseError {
                    message: format!("unknown oob-resolution bucket: {other}"),
                }),
            }).transpose()?;
            ResolutionRoute::OobResolution {
                bucket,
                cursor: params.get_usize("cursor"),
            }
        }
        "tag-canonicity" => {
            let tag_name = segments.get(2).ok_or_else(|| RouteParseError {
                message: "tag-canonicity requires a tag name".into(),
            })?;
            let zone = params.get_string("zone")
                .and_then(|s| Zone::from_str(&s))
                .ok_or_else(|| RouteParseError {
                    message: "tag-canonicity requires a valid zone parameter".into(),
                })?;
            ResolutionRoute::TagCanonicity {
                tag_name: (*tag_name).to_string(),
                zone,
                cluster: params.get_usize("cluster"),
            }
        }
        "inconsistent-album-artist" => {
            let tag_name = segments.get(2).ok_or_else(|| RouteParseError {
                message: "inconsistent-album-artist requires a tag name".into(),
            })?;
            ResolutionRoute::InconsistentAlbumArtist {
                tag_name: (*tag_name).to_string(),
                cluster: params.get_usize("cluster"),
            }
        }
        "compound-split" => {
            let tag_name = segments.get(2).ok_or_else(|| RouteParseError {
                message: "compound-split requires a tag name".into(),
            })?;
            let zone = params.get_string("zone")
                .and_then(|s| Zone::from_str(&s))
                .ok_or_else(|| RouteParseError {
                    message: "compound-split requires a valid zone parameter".into(),
                })?;
            ResolutionRoute::CompoundSplit {
                tag_name: (*tag_name).to_string(),
                zone,
                safe_mode: params.get_string("safe").map_or(false, |s| s == "true"),
                cluster: params.get_usize("cluster"),
            }
        }
        "missing-album" => ResolutionRoute::MissingAlbum {
            group: params.get_usize("group"),
        },
        "disc-extraction" => ResolutionRoute::DiscExtraction {
            group: params.get_usize("group"),
        },
        "redundant-duplicates" => ResolutionRoute::RedundantDuplicates {
            group: params.get_usize("group"),
        },
        "deploy-conflicts" => ResolutionRoute::DeployConflicts {
            group: params.get_usize("group"),
        },
        "metadata-duplicates" => ResolutionRoute::MetadataDuplicates {
            group: params.get_usize("group"),
        },
        "same-recording" => ResolutionRoute::SameRecording {
            group: params.get_usize("group"),
        },
        other => {
            return Err(RouteParseError {
                message: format!("unknown resolution type: {other}"),
            });
        }
    };
    Ok(Route::Resolution(route))
}

// ============================================================================
// External match overlay URL helpers
// ============================================================================

fn external_match_overlay_to_url(r: &ExternalMatchRoute, path: &mut String, params: &mut QueryParams) {
    path.push_str("/external-matches/");
    match r {
        ExternalMatchRoute::AcoustidBrowse { confidence, cursor } => {
            path.push_str("acoustid/");
            path.push_str(confidence.as_str());
            params.set_usize("cursor", *cursor);
        }
        ExternalMatchRoute::ReleaseReview { filter, cursor } => {
            path.push_str("review/");
            path.push_str(filter.as_str());
            params.set_usize("cursor", *cursor);
        }
        ExternalMatchRoute::ReleaseDetail { release_id, cursor } => {
            path.push_str("review/release/");
            path.push_str(release_id);
            params.set_usize("cursor", *cursor);
        }
    }
}

// ============================================================================
// Query parameter helpers
// ============================================================================

/// Lightweight query parameter builder/parser.
struct QueryParams {
    pairs: Vec<(String, String)>,
}

impl QueryParams {
    fn new() -> Self {
        Self { pairs: Vec::new() }
    }

    fn from_pairs(pairs: &[(String, String)]) -> Self {
        Self {
            pairs: pairs.to_vec(),
        }
    }

    fn set_str(&mut self, key: &str, value: Option<&str>) {
        if let Some(v) = value {
            self.pairs.push((key.to_string(), v.to_string()));
        }
    }

    fn set_usize(&mut self, key: &str, value: Option<usize>) {
        if let Some(v) = value {
            self.pairs.push((key.to_string(), v.to_string()));
        }
    }

    fn set_focus(&mut self, key: &str, value: Option<FocusTarget>) {
        if let Some(f) = value {
            let s = match f {
                FocusTarget::List => "list",
                FocusTarget::Buttons => "buttons",
            };
            self.pairs.push((key.to_string(), s.to_string()));
        }
    }

    fn get_string(&self, key: &str) -> Option<String> {
        self.pairs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
    }

    fn get_usize(&self, key: &str) -> Option<usize> {
        self.get_string(key).and_then(|s| s.parse().ok())
    }

    fn get_focus(&self, key: &str) -> Option<FocusTarget> {
        self.get_string(key).and_then(|s| match s.as_str() {
            "list" => Some(FocusTarget::List),
            "buttons" => Some(FocusTarget::Buttons),
            _ => None,
        })
    }

    fn to_query_string(&self) -> String {
        self.pairs
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&")
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Round-trip: to_url → from_url → to_url produces same output.
    fn assert_round_trip(route: &Route) {
        let url = route.to_url();
        let (path, query) = parse_url_for_test(&url);
        let parsed = Route::from_url(&path, &query).unwrap_or_else(|e| {
            panic!("failed to parse route from url '{url}': {e}");
        });
        assert_eq!(route, &parsed, "round-trip failed for url: {url}");
    }

    /// Split a URL into path and query pairs for testing.
    fn parse_url_for_test(url: &str) -> (String, Vec<(String, String)>) {
        if let Some((path, query_str)) = url.split_once('?') {
            let pairs: Vec<(String, String)> = query_str
                .split('&')
                .filter(|s| !s.is_empty())
                .filter_map(|pair| {
                    let (k, v) = pair.split_once('=')?;
                    Some((k.to_string(), v.to_string()))
                })
                .collect();
            (path.to_string(), pairs)
        } else {
            (url.to_string(), vec![])
        }
    }

    // -- Lateral views --

    #[test]
    fn round_trip_config_bare() {
        assert_round_trip(&Route::Config(ConfigRoute::default()));
    }

    #[test]
    fn round_trip_config_full() {
        assert_round_trip(&Route::Config(ConfigRoute {
            cursor: Some(5),
            editing: Some("mb-base-url".into()),
            focus: Some(FocusTarget::Buttons),
        }));
    }

    #[test]
    fn round_trip_search() {
        assert_round_trip(&Route::Search(SearchRoute {
            mode: Some(SearchMode::Results),
            query: Some("artist:contains:Bach".into()),
            cursor: Some(3),
            focused_condition: None,
            field_focus: None,
        }));
    }

    #[test]
    fn round_trip_files_with_path() {
        assert_round_trip(&Route::Files(FilesRoute {
            path: Some("Classical/Bach".into()),
            search: Some("fugue".into()),
            panel: Some("config".into()),
            panel_field: Some(2),
        }));
    }

    #[test]
    fn round_trip_files_bare() {
        assert_round_trip(&Route::Files(FilesRoute::default()));
    }

    #[test]
    fn round_trip_health() {
        assert_round_trip(&Route::Health(HealthRoute { cursor: Some(7) }));
    }

    #[test]
    fn round_trip_history() {
        assert_round_trip(&Route::History(HistoryRoute { session: Some(42) }));
    }

    #[test]
    fn round_trip_transaction() {
        assert_round_trip(&Route::Transaction(TransactionRoute {
            cursor: Some(1),
            focus: Some(FocusTarget::List),
        }));
    }

    #[test]
    fn round_trip_deploy() {
        use crate::domain_types::DeployTab;
        assert_round_trip(&Route::Deploy(DeployRoute {
            tab: Some(DeployTab::Conflicts),
            scroll: Some(5),
        }));
    }

    #[test]
    fn round_trip_external_matches() {
        assert_round_trip(&Route::ExternalMatches(ExternalMatchesRoute {
            cursor: Some(3),
        }));
    }

    // -- Overlays --

    #[test]
    fn round_trip_tag_editor_single() {
        assert_round_trip(&Route::TagEditor(TagEditorRoute {
            inodes: vec![12345],
            mode: None,
            item: None,
            field: Some(3),
            editing: Some("value".into()),
        }));
    }

    #[test]
    fn round_trip_tag_editor_bulk() {
        assert_round_trip(&Route::TagEditor(TagEditorRoute {
            inodes: vec![123, 456, 789],
            mode: Some("aggregate".into()),
            item: Some(2),
            field: Some(5),
            editing: None,
        }));
    }

    #[test]
    fn round_trip_transaction_review() {
        assert_round_trip(&Route::TransactionReview(TransactionReviewRoute {
            cursor: Some(1),
            focus: Some(FocusTarget::Buttons),
        }));
    }

    #[test]
    fn round_trip_packing_browser() {
        assert_round_trip(&Route::PackingBrowser(PackingBrowserRoute {
            category: "album-category".into(),
            cursor: Some(5),
        }));
    }

    #[test]
    fn round_trip_knot_browser() {
        assert_round_trip(&Route::KnotBrowser(KnotBrowserRoute {
            id: "abc123".into(),
            cursor: Some(3),
        }));
    }

    // -- Resolutions --

    #[test]
    fn round_trip_resolution_missing_files_restorable() {
        assert_round_trip(&Route::Resolution(ResolutionRoute::MissingFilesRestorable {
            cursor: Some(3),
            focus: Some(FocusTarget::Buttons),
        }));
    }

    #[test]
    fn round_trip_resolution_missing_files_permanent() {
        assert_round_trip(&Route::Resolution(ResolutionRoute::MissingFilesPermanent {
            cursor: Some(1),
        }));
    }

    #[test]
    fn round_trip_resolution_tag_canonicity() {
        assert_round_trip(&Route::Resolution(ResolutionRoute::TagCanonicity {
            tag_name: "artist".into(),
            zone: Zone::Corpus,
            cluster: Some(2),
        }));
    }

    #[test]
    fn round_trip_resolution_inconsistent_album_artist() {
        assert_round_trip(&Route::Resolution(ResolutionRoute::InconsistentAlbumArtist {
            tag_name: "albumartist".into(),
            cluster: Some(5),
        }));
    }

    #[test]
    fn round_trip_resolution_compound_split() {
        assert_round_trip(&Route::Resolution(ResolutionRoute::CompoundSplit {
            tag_name: "genre".into(),
            zone: Zone::Corpus,
            safe_mode: true,
            cluster: Some(3),
        }));
    }

    #[test]
    fn round_trip_resolution_group_review_variants() {
        let cases = vec![
            ResolutionRoute::RedundantDuplicates { group: Some(1) },
            ResolutionRoute::DeployConflicts { group: Some(2) },
            ResolutionRoute::MetadataDuplicates { group: None },
            ResolutionRoute::SameRecording { group: Some(0) },
        ];
        for r in cases {
            assert_round_trip(&Route::Resolution(r));
        }
    }

    #[test]
    fn round_trip_all_simple_resolutions() {
        let cases = vec![
            ResolutionRoute::MissingDirectories { cursor: Some(1) },
            ResolutionRoute::CorruptFiles { cursor: Some(2) },
            ResolutionRoute::LosslessRemux { cursor: None },
            ResolutionRoute::SubparDuplicates { cursor: Some(4) },
            ResolutionRoute::DirectoryCluster { cluster: Some(7), cursor: Some(3) },
            ResolutionRoute::MovedFiles { cursor: Some(1) },
            ResolutionRoute::OobResolution { bucket: None, cursor: Some(5) },
            ResolutionRoute::OobResolution { bucket: Some(ConflictBucket::MtimeOnly), cursor: None },
            ResolutionRoute::OobResolution { bucket: Some(ConflictBucket::DbOnly), cursor: Some(2) },
            ResolutionRoute::OobResolution { bucket: Some(ConflictBucket::DiskOnly), cursor: Some(1) },
            ResolutionRoute::OobResolution { bucket: Some(ConflictBucket::Conflict), cursor: None },
            ResolutionRoute::MissingAlbum { group: Some(1) },
            ResolutionRoute::DiscExtraction { group: Some(0) },
        ];
        for r in cases {
            assert_round_trip(&Route::Resolution(r));
        }
    }

    // -- External match overlays --

    #[test]
    fn round_trip_external_match_acoustid_browse() {
        assert_round_trip(&Route::ExternalMatchOverlay(ExternalMatchRoute::AcoustidBrowse {
            confidence: AcoustidConfidence::High,
            cursor: Some(3),
        }));
    }

    #[test]
    fn round_trip_external_match_acoustid_all() {
        assert_round_trip(&Route::ExternalMatchOverlay(ExternalMatchRoute::AcoustidBrowse {
            confidence: AcoustidConfidence::All,
            cursor: None,
        }));
    }

    #[test]
    fn round_trip_external_match_release_review() {
        assert_round_trip(&Route::ExternalMatchOverlay(ExternalMatchRoute::ReleaseReview {
            filter: ReleaseReviewFilter::Perfect,
            cursor: Some(0),
        }));
    }

    #[test]
    fn round_trip_external_match_release_review_all() {
        assert_round_trip(&Route::ExternalMatchOverlay(ExternalMatchRoute::ReleaseReview {
            filter: ReleaseReviewFilter::All,
            cursor: None,
        }));
    }

    #[test]
    fn round_trip_external_match_release_detail() {
        assert_round_trip(&Route::ExternalMatchOverlay(ExternalMatchRoute::ReleaseDetail {
            release_id: "abc12345-6789-0123-4567-890abcdef012".into(),
            cursor: Some(2),
        }));
    }

    #[test]
    fn url_shape_external_match_acoustid() {
        let url = Route::ExternalMatchOverlay(ExternalMatchRoute::AcoustidBrowse {
            confidence: AcoustidConfidence::High,
            cursor: Some(3),
        })
        .to_url();
        assert_eq!(url, "/external-matches/acoustid/high?cursor=3");
    }

    #[test]
    fn url_shape_external_match_release_review() {
        let url = Route::ExternalMatchOverlay(ExternalMatchRoute::ReleaseReview {
            filter: ReleaseReviewFilter::FullMatch,
            cursor: None,
        })
        .to_url();
        assert_eq!(url, "/external-matches/review/full-match");
    }

    #[test]
    fn url_shape_external_match_release_detail() {
        let url = Route::ExternalMatchOverlay(ExternalMatchRoute::ReleaseDetail {
            release_id: "abc123".into(),
            cursor: Some(0),
        })
        .to_url();
        assert_eq!(url, "/external-matches/review/release/abc123?cursor=0");
    }

    // -- Edge cases --

    #[test]
    fn parse_empty_path_defaults_to_health() {
        let route = Route::from_url("", &[]).unwrap();
        assert_eq!(route, Route::Health(HealthRoute::default()));
    }

    #[test]
    fn parse_hash_prefix_stripped() {
        let route = Route::from_url("#health", &[("cursor".into(), "3".into())]).unwrap();
        assert_eq!(route, Route::Health(HealthRoute { cursor: Some(3) }));
    }

    #[test]
    fn parse_unknown_route_errors() {
        assert!(Route::from_url("/nonexistent", &[]).is_err());
    }

    #[test]
    fn parse_tags_no_inodes_errors() {
        assert!(Route::from_url("/tags", &[]).is_err());
    }

    // -- URL shape tests --

    #[test]
    fn url_shape_config() {
        let url = Route::Config(ConfigRoute {
            cursor: Some(5),
            editing: Some("mb-base-url".into()),
            focus: Some(FocusTarget::Buttons),
        })
        .to_url();
        assert_eq!(url, "/config?cursor=5&editing=mb-base-url&focus=buttons");
    }

    #[test]
    fn url_shape_resolve_tag_canonicity() {
        let url = Route::Resolution(ResolutionRoute::TagCanonicity {
            tag_name: "artist".into(),
            zone: Zone::Corpus,
            cluster: Some(2),
        })
        .to_url();
        assert_eq!(url, "/resolve/tag-canonicity/artist?zone=corpus&cluster=2");
    }

    #[test]
    fn url_shape_tags_single() {
        let url = Route::TagEditor(TagEditorRoute {
            inodes: vec![12345],
            mode: None,
            item: None,
            field: Some(3),
            editing: Some("value".into()),
        })
        .to_url();
        assert_eq!(url, "/tags/12345?field=3&editing=value");
    }

    #[test]
    fn url_shape_tags_bulk() {
        let url = Route::TagEditor(TagEditorRoute {
            inodes: vec![123, 456],
            mode: Some("aggregate".into()),
            item: Some(1),
            field: None,
            editing: None,
        })
        .to_url();
        assert_eq!(url, "/tags?inodes=123,456&mode=aggregate&item=1");
    }
}
