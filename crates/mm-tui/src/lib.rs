//! Terminal User Interface
//!
//! ## Module Organization
//! - `active_view.rs` - ActiveView enum (single source of truth for view + state)
//! - `types.rs` - ProgressStatsUpdater trait
//! - `action_handlers/` - View-specific action processing
//! - `tag_editor_ops.rs` - Tag editor launching and navigation
//! - `tick.rs` - Per-frame update logic for each view
//!
//! ## Adding New Views
//! 1. Add variant to `ActiveView` in `active_view.rs`
//! 2. Add ViewAction variant in `active_view.rs`
//! 3. Add key dispatch + action handler in `action_handlers/`
//! 4. Add render case in `render.rs`

pub(crate) mod action_handlers;
pub(crate) mod active_view;
pub(crate) mod input;
pub(crate) mod packing_colors;
mod rpc;
mod suspended_views;
mod tag_editor_ops;
mod tick;
mod types;

pub mod operator_decisions;
pub mod tabbed_transaction_review;
pub mod transaction_review;

pub mod acoustid_browse;
pub mod artist_plural_modal;
pub mod compound_split_v2;
pub mod config_editor;
pub mod corrupt_file_modal;
pub mod deploy_modal;
pub mod directory_cluster_modal;
pub mod disc_extraction_modal;
pub mod external_match_view;
pub mod eye;
pub mod helpers;
pub mod insights_view;
pub mod knot_browser;
pub mod manual_review_modal;
pub mod missing_album_modal;
pub mod missing_directory_modal;
pub mod missing_file_modal;
pub mod moved_file_modal;
pub mod oob_conflict_modal;
pub mod progress_screen;
pub mod progressive_worker;
pub mod release_packing_browser;
pub mod release_review;
pub mod render;
pub mod lossless_remux_modal;
pub mod startup;
pub mod subpar_duplicate_modal;
pub mod tag_canonicity_v2;
pub mod tag_editor;
pub mod tag_search;
pub mod tree_browser;
pub mod wait_state;
pub mod widgets;

// Re-export for convenience
pub(crate) use active_view::{
    ActiveView, ExitConfirmAction, ExitConfirmModalState,
    LoginAction, LoginField, LoginScreenState,
    ViewAction,
};
use types::ProgressStatsUpdater;

use anyhow::Result;
use crossterm::{
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, MouseButton, MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use input::InputAction;
use ratatui::{backend::CrosstermBackend, Frame, Terminal};
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use mm_meta::config::Config;
use mm_meta::paths::PathResolver;
use mm_meta::witch_types::WitchStatus;

pub use startup_socket::StartupSocket;

// ============================================================================
// Application State
// ============================================================================

/// Main application state
pub(crate) struct App {
    should_quit: bool,
    pub(crate) status_message: Option<String>,

    /// Error popup overlay — dismissed on any key press.
    pub(crate) error_popup: Option<String>,

    /// The active view and its state. One variant is active at a time.
    pub(crate) view: ActiveView,

    // RPC bridge to the Witch server (async socket on background thread).
    rpc: rpc::RpcHandle,
    session_token: Option<mm_meta::auth::SessionToken>,
    cached_config: Option<Arc<Config>>,

    /// Client-side path resolver for root-relative ↔ absolute path conversion.
    /// Constructed from config after login.
    pub(crate) resolver: PathResolver,

    // View stack for push/pop navigation — stores Routes, pop navigates fresh.
    pub(crate) view_stack: Vec<mm_ui::route::Route>,

    /// Last lateral view the user was on. Used for returning after modal flows.
    pub(crate) last_lateral_view: widgets::LateralView,

    /// Whether startup maintenance (schema reconciliation, vacuum) has completed.
    /// Set to true once WitchStartupState transitions to Ready.
    startup_complete: bool,

    /// Generation counters from last frame — used to detect events by diffing
    /// against the current WitchStatus each frame.
    prev_mutations_generation: u64,
    prev_computations_generation: u64,
    prev_error_generation: u64,
    prev_config_generation: u64,

    /// Cached WitchStatus — populated by server-pushed events.
    cached_status: WitchStatus,

    /// Terminal image rendering: picker for protocol detection + image cache.
    pub(crate) art_picker: widgets::AlbumArtPicker,
    pub(crate) art_cache: widgets::AlbumArtCache,

    /// Click targets for titlebar tabs, populated during render.
    pub(crate) tab_click_rects: Vec<(widgets::LateralView, ratatui::layout::Rect)>,

    /// Current route — synchronized after every view transition and state mutation.
    /// Used for Route-driven navigation and URL display.
    pub(crate) current_route: Option<mm_ui::route::Route>,

    /// Set when the server returns InvalidSession — main loop breaks and re-enters login.
    session_expired: bool,

    /// Throttle for `refresh_active_view_data`: earliest time the next refresh is allowed.
    /// Prevents a tight blocking-query loop when the Witch is broadcasting rapid status events
    /// (each query blocks while events accumulate → immediate re-query → hang).
    next_refresh_allowed_at: std::time::Instant,
}

impl App {
    /// Create a new App with an RPC handle and session token.
    fn new(
        rpc: rpc::RpcHandle,
        session_token: mm_meta::auth::SessionToken,
        art_picker: widgets::AlbumArtPicker,
    ) -> Self {
        // Pre-fetch status and config via the RPC handle (avoids placeholder issues).
        let token = session_token.clone();

        let cached_status: WitchStatus = {
            let req = mm_meta::wire::WireRequest::Authenticated {
                request_id: 0,
                token: token.clone(),
                body: Box::new(mm_meta::protocol::AuthenticatedBody::Query(
                    mm_meta::protocol::QueryPayload::Status,
                )),
            };
            let resp = rpc.send_blocking(req).expect("initial status query");
            match resp {
                mm_meta::wire::WireResponse::Authenticated { result, .. } => match *result {
                    Ok(mm_meta::protocol::AuthenticatedResponse::Query(qr)) => match *qr {
                        mm_meta::protocol::QueryResponse::Status(s) => s,
                        _ => panic!("unexpected query response"),
                    },
                    _ => panic!("status query failed"),
                },
                _ => panic!("unexpected wire response"),
            }
        };

        let config: Config = {
            let req = mm_meta::wire::WireRequest::Authenticated {
                request_id: 0,
                token: token.clone(),
                body: Box::new(mm_meta::protocol::AuthenticatedBody::Query(
                    mm_meta::protocol::QueryPayload::Config,
                )),
            };
            let resp = rpc.send_blocking(req).expect("initial config query");
            match resp {
                mm_meta::wire::WireResponse::Authenticated { result, .. } => match *result {
                    Ok(mm_meta::protocol::AuthenticatedResponse::Query(qr)) => match *qr {
                        mm_meta::protocol::QueryResponse::Config(c) => *c,
                        _ => panic!("unexpected query response"),
                    },
                    _ => panic!("config query failed"),
                },
                _ => panic!("unexpected wire response"),
            }
        };

        let resolver = PathResolver::from_config(&config);
        let cached_config = Some(Arc::new(config));

        Self {
            should_quit: false,
            status_message: None,
            error_popup: None,
            view: ActiveView::Insights(insights_view::InsightsViewState::new(
                insights_view::InsightsViewData::new(),
            )),
            rpc,
            session_token: Some(session_token),
            cached_config,
            resolver,
            view_stack: Vec::new(),
            last_lateral_view: widgets::LateralView::Health,
            startup_complete: false,
            prev_mutations_generation: 0,
            prev_computations_generation: 0,
            prev_error_generation: 0,
            prev_config_generation: 0,
            cached_status,
            art_picker,
            art_cache: widgets::AlbumArtCache::new(),
            tab_click_rects: Vec::new(),
            current_route: None,
            session_expired: false,
            next_refresh_allowed_at: std::time::Instant::now(),
        }
    }

    /// Get a snapshot of the current config from the Witch (cached).
    pub(crate) fn config(&mut self) -> Arc<Config> {
        if let Some(ref c) = self.cached_config {
            return Arc::clone(c);
        }
        let arc = Arc::new(self.query(mm_meta::protocol::ConfigQuery));
        self.cached_config = Some(Arc::clone(&arc));
        arc
    }

    /// Return a reference to the cached WitchStatus (populated by server-pushed events).
    pub(crate) fn witch_status(&self) -> &WitchStatus {
        &self.cached_status
    }

    /// Whether the Transaction tab should be visible in the lateral view ring.
    fn transactions_open(&mut self) -> bool {
        self.config().opinions.leave_transactions_open
    }

    /// Whether leave-transactions-open mode is active (alias for readability in control flow).
    fn open_txn_mode(&mut self) -> bool {
        self.config().opinions.leave_transactions_open
    }

    /// Return to the last lateral view the user was on.
    fn return_to_last_lateral_view(&mut self) {
        self.start_lateral_view(self.last_lateral_view);
    }

    fn handle_input(&mut self, action: InputAction) {
        // Error popup eats any key to dismiss
        if self.error_popup.is_some() {
            self.error_popup = None;
            return;
        }

        // Pre-dispatch: intercept CycleNext/CyclePrev for lateral views that
        // don't need to handle them as domain actions (7 of 9 views).
        if let Some(lv) = self.view.lateral_view() {
            if !self.view.wants_raw_cycle() {
                match action {
                    InputAction::CycleNext => { self.handle_lateral_cycle(lv, true); return; }
                    InputAction::CyclePrev => { self.handle_lateral_cycle(lv, false); return; }
                    _ => {}
                }
            }
        }

        // Macro for views whose handle_input returns Option<DomainAction>:
        // None → ViewAction::None, Some(a) → ViewAction::$variant(a)
        macro_rules! dispatch_input {
            ($variant:ident, $state:expr) => {
                match $state.handle_input(&action) {
                    Some(a) => ViewAction::$variant(a),
                    None => ViewAction::None,
                }
            };
        }

        // Macro for views whose handle_input still returns a concrete action type
        // (ConfigEditor, CorpusBrowser — they keep CycleNext/CyclePrev variants)
        macro_rules! dispatch_input_raw {
            ($variant:ident, $state:expr) => {
                ViewAction::$variant($state.handle_input(&action))
            };
        }

        // Phase 1: borrow view, produce view action
        let view_action = match &mut self.view {
            ActiveView::StartupMaintenance => ViewAction::None,
            ActiveView::Progress { .. } => ViewAction::None,
            ActiveView::ProgressiveWork(_) => ViewAction::None,
            ActiveView::ConfigEditor(s) => dispatch_input!(ConfigEditor, s),
            ActiveView::Insights(ref mut s) => dispatch_input!(Insights, s),
            ActiveView::CorpusBrowser(s) => dispatch_input!(CorpusBrowser, s),
            ActiveView::TagSearch(s) => dispatch_input!(TagSearch, s),
            ActiveView::TabbedTransactionReview(ref mut s) => dispatch_input!(TabbedTransactionReview, s),
            ActiveView::ExitConfirm(state) => {
                let a = match action {
                    InputAction::NavLeft | InputAction::FocusLeft => {
                        state.selected = state.selected.saturating_sub(1);
                        ExitConfirmAction::None
                    }
                    InputAction::NavRight | InputAction::FocusRight => {
                        state.selected = (state.selected + 1).min(2);
                        ExitConfirmAction::None
                    }
                    InputAction::NavDown => {
                        // Jump to shutdown option
                        state.selected = 2;
                        ExitConfirmAction::None
                    }
                    InputAction::NavUp => {
                        // Jump back to top row, preserving left/right
                        if state.selected == 2 {
                            state.selected = 0;
                        }
                        ExitConfirmAction::None
                    }
                    InputAction::Confirm | InputAction::Toggle => {
                        match state.selected {
                            0 => ExitConfirmAction::Quit,
                            2 => ExitConfirmAction::QuitAndShutdown,
                            _ => ExitConfirmAction::Cancel,
                        }
                    }
                    InputAction::Cancel => ExitConfirmAction::Cancel,
                    _ => ExitConfirmAction::None,
                };
                ViewAction::ExitConfirm(a)
            }
            ActiveView::UnifiedTagEditor(s) => dispatch_input_raw!(UnifiedTagEditor, s),
            ActiveView::Deploy(ref mut s) => dispatch_input!(Deploy, s),
            ActiveView::ExternalMatches(ref mut s) => dispatch_input!(ExternalMatches, s),
            ActiveView::MissingFileResolution(s) => dispatch_input_raw!(MissingFileResolution, s),
            ActiveView::MissingDirectoryResolution(s) => dispatch_input!(MissingDirectoryResolution, s),
            ActiveView::CorruptFileResolution(s) => dispatch_input!(CorruptFileResolution, s),
            ActiveView::LosslessRemuxResolution(s) => dispatch_input_raw!(LosslessRemuxResolution, s),
            ActiveView::SubparDuplicateResolution(s) => dispatch_input!(SubparDuplicateResolution, s),
            ActiveView::ArtistPluralResolution(s) => dispatch_input!(ArtistPluralResolution, s),
            ActiveView::DirectoryClusterResolution(ref mut s) => {
                match s.handle_input_with_items(&action) {
                    Some(a) => ViewAction::DirectoryClusterResolution(a),
                    None => ViewAction::None,
                }
            }
            ActiveView::MovedFileAcknowledge(s) => dispatch_input!(MovedFileAcknowledge, s),
            ActiveView::OobResolution(s) => dispatch_input_raw!(OobResolution, s),
            ActiveView::ReleasePackingBrowser(s) => dispatch_input_raw!(ReleasePackingBrowser, s),
            ActiveView::KnotBrowser(s) => dispatch_input_raw!(KnotBrowser, s),
            ActiveView::AcoustidBrowse(s) => dispatch_input_raw!(AcoustidBrowse, s),
            ActiveView::ReleaseReview(s) => dispatch_input_raw!(ReleaseReview, s),
            ActiveView::TagCanonicityResolution(ref mut s) => dispatch_input!(TagCanonicityResolution, s),
            ActiveView::CompoundTagSplitResolution(ref mut s) => dispatch_input!(CompoundTagSplitResolution, s),
            ActiveView::MissingAlbumSingleResolution(ref mut s) => dispatch_input!(MissingAlbumSingleResolution, s),
            ActiveView::DiscExtractionResolution(ref mut s) => dispatch_input!(DiscExtractionResolution, s),
            ActiveView::ManualReviewResolution(ref mut s) => dispatch_input!(ManualReviewResolution, s),
            ActiveView::TransactionReview(s) => dispatch_input_raw!(TransactionReview, s),
            ActiveView::LoginScreen(s) => {
                let a = match action {
                    InputAction::NavDown | InputAction::FocusRight => {
                        s.focus_next();
                        s.error_message = None;
                        LoginAction::None
                    }
                    InputAction::NavUp | InputAction::FocusLeft => {
                        s.focus_next();
                        s.error_message = None;
                        LoginAction::None
                    }
                    InputAction::Confirm => LoginAction::AttemptLogin,
                    _ => {
                        match s.focused {
                            LoginField::Username => { s.username.handle_input(&action); }
                            LoginField::Password => { s.password.handle_input(&action); }
                        }
                        s.error_message = None;
                        LoginAction::None
                    }
                };
                ViewAction::LoginScreen(a)
            }
        };

        // Post-dispatch: if no view produced a domain action and the input was Cancel,
        // treat it as quit for lateral views. Views that handle Cancel as a domain
        // action (ConfigEditor→Discard, CorpusBrowser→Cancel, TagSearch→Cancel)
        // produce Some(action), so the fallback never fires for them.
        if matches!(&view_action, ViewAction::None) && matches!(action, InputAction::Cancel)
            && self.view.lateral_view().is_some()
        {
            self.handle_request_quit();
            return;
        }

        // Phase 2: dispatch with confirmation flag
        let is_confirmation = matches!(action, InputAction::Confirm);
        self.dispatch_action(view_action, is_confirmation);

        // Phase 3: handle pending DirectoryBrowser actions (expand, search)
        self.handle_pending_browser_action();

        // Phase 4: sync route from view state
        self.sync_route();
    }

    /// Dispatch any pending DirectoryBrowser action (RequestExpand, RequestSearch).
    ///
    /// Called after input dispatch. Separated to avoid borrow conflicts between
    /// `self.view` (take action) and `self.query()` (socket).
    fn handle_pending_browser_action(&mut self) {
        let pending = match self.view {
            ActiveView::CorpusBrowser(ref mut browser_state) => browser_state.take_pending_browser_action(),
            _ => None,
        };
        let Some(action) = pending else { return };
        match action {
            mm_ui::directory_browser::BrowserAction::RequestExpand(path) => {
                let children = self.query(mm_meta::domain_queries::GetDirectoryListing {
                    zone: mm_meta::db_types::Zone::Corpus,
                    parent: Some(path.clone()),
                });
                if let ActiveView::CorpusBrowser(ref mut bs) = self.view {
                    bs.browser_mut().populate_children(&path, children);
                }
            }
            mm_ui::directory_browser::BrowserAction::RequestSearch(query) => {
                let results = self.query(mm_meta::domain_queries::SearchCorpusFiles {
                    query,
                    limit: 100,
                });
                let matching_paths: std::collections::HashSet<String> = results.iter()
                    .map(|r| r.path.clone())
                    .collect();
                if let ActiveView::CorpusBrowser(ref mut bs) = self.view {
                    // Set filter on the browser directly (variant stores it, browser stores display)
                    bs.browser_mut().set_path_filter(matching_paths);
                }
            }
            _ => {}
        }
    }

    /// Check if there are any pending operations (Witch work).
    pub(crate) fn has_pending_operations(&self) -> bool {
        self.witch_status().has_pending
    }

    /// Re-fetch data for the currently active view.
    ///
    /// Called when a generation counter bumps (computations or mutations completed)
    /// to refresh the active view's data without a full view restart.
    fn refresh_active_view_data(&mut self) {
        match &self.view {
            ActiveView::Insights(_) => {
                let insights_data = self.query(mm_meta::domain_queries::GetInsights);
                if let ActiveView::Insights(ref mut s) = self.view {
                    let rebuilt = s.data.update(
                        Some(&self.cached_status.work),
                        Some(insights_data),
                        &self.cached_status.handled_decision_kinds,
                    );
                    if rebuilt {
                        s.interaction.list.clamp_cursor(&s.data.flat_items);
                    }
                }
            }
            ActiveView::ExternalMatches(_) => {
                let ext_data = self.query(mm_meta::domain_queries::GetExternalMatches);
                if let ActiveView::ExternalMatches(ref mut s) = self.view {
                    s.data.update(ext_data);
                    s.interaction.clamp_to_data(&s.data.flat_items);
                    s.data.rebuild_items();
                    s.interaction.clamp_to_data(&s.data.flat_items);
                }
            }
            ActiveView::Deploy(ref s) if matches!(s.data, deploy_modal::DeployViewData::UpToDate { .. }) => {
                let status = self.query(mm_meta::domain_queries::GetDeployStatus);
                if let ActiveView::Deploy(ref mut s) = self.view {
                    if let deploy_modal::DeployViewData::UpToDate { ref mut library_file_counts } = s.data {
                        *library_file_counts = status.library_file_counts;
                    }
                }
            }
            ActiveView::CorpusBrowser(_) => {
                let config = self.config();
                let packing = self.query(mm_meta::domain_queries::GetPackingDirs);
                if let ActiveView::CorpusBrowser(ref mut browser_state) = self.view {
                    let tree_browser::BrowserVariant::CorpusBrowser(ref mut v) = browser_state.variant_mut();
                    let file_paths: std::collections::HashSet<String> = packing.file_paths.iter()
                        .filter_map(|p| p.strip_prefix(&config.storage_root).ok())
                        .map(|p| p.to_string_lossy().to_string())
                        .collect();
                    let dir_categories: std::collections::HashMap<String, mm_meta::signals::packing_category::PackingCategory> = packing.dir_categories.iter()
                        .filter_map(|(p, &cat)| {
                            p.strip_prefix(&config.storage_root).ok().map(|rel| (rel.to_string_lossy().to_string(), cat))
                        })
                        .collect();
                    v.set_packing_markers(file_paths, dir_categories);
                }
            }
            _ => {}
        }
    }

    /// Start the health view.
    pub(crate) fn start_health_view(&mut self) {
        self.clear_view_stack();
        self.last_lateral_view = widgets::LateralView::Health;
        let insights_data = self.query(mm_meta::domain_queries::GetInsights);
        let mut data = insights_view::InsightsViewData::new();
        data.update(
            Some(&self.cached_status.work),
            Some(insights_data),
            &self.cached_status.handled_decision_kinds,
        );
        self.view = ActiveView::Insights(insights_view::InsightsViewState::new(data));
        self.sync_route();
    }

    /// Start the configured default view (post-startup landing screen).
    pub(crate) fn start_default_view(&mut self) {
        let config = self.config();
        let default_view = config.opinions.startup.default_view;
        match default_view {
            mm_meta::config::StartupView::Health => self.start_health_view(),
            mm_meta::config::StartupView::Search => self.start_tag_search(),
            mm_meta::config::StartupView::Browser => self.start_corpus_browser(),
            mm_meta::config::StartupView::ExternalMatches => self.start_external_matches_view(),
        }
    }

    /// Abort current operation and return to health view with a status message.
    pub(crate) fn abort_to_health(&mut self, message: String) {
        self.status_message = Some(message);
        self.start_health_view();
    }

    /// Update Witch stats on a progress state that implements the stats setter methods.
    pub(crate) fn update_progress_stats<T>(&self, state: &mut T)
    where
        T: ProgressStatsUpdater,
    {
        state.set_db_queue_depth(self.witch_status().db_queue_depth);
    }

    pub(crate) fn start_tag_search(&mut self) {
        self.last_lateral_view = widgets::LateralView::Search;
        self.view = ActiveView::TagSearch(tag_search::TagSearchState::new());
        self.sync_route();
    }

    /// Start the external matches lateral view.
    pub(crate) fn start_external_matches_view(&mut self) {
        self.last_lateral_view = widgets::LateralView::ExternalMatches;
        let ws = self.witch_status();
        let fetch_active = ws.is_external_fetch_active;
        let has_api_key = ws.has_acoustid_api_key;
        let cover_art_active = ws.is_cover_art_fetch_active;
        let deezer_active = ws.is_deezer_fetch_active;
        let config = self.config();
        let singles_before_incompletes = config.opinions.release_packing.singles_before_incompletes;
        let mut data = external_match_view::ExternalMatchesViewData::new(
            fetch_active,
            has_api_key,
            singles_before_incompletes,
            cover_art_active,
            deezer_active,
        );
        let ext_data = self.query(mm_meta::domain_queries::GetExternalMatches);
        data.update(ext_data);
        self.view = ActiveView::ExternalMatches(external_match_view::ExternalMatchesViewState::new(data));
        self.sync_route();
    }

    /// Start the lateral view identified by the given variant.
    pub(crate) fn start_lateral_view(&mut self, view: widgets::LateralView) {
        self.navigate_to(view.to_default_route());
    }

    /// Route-driven navigation: construct the appropriate view from a Route.
    ///
    /// For lateral routes, this queries data, constructs the view state,
    /// then restores cursor/scroll/tab position from the Route parameters.
    pub(crate) fn navigate_to(&mut self, route: mm_ui::route::Route) {
        use mm_ui::route::Route;
        match route {
            Route::Health(ref r) => {
                self.start_health_view();
                if let ActiveView::Insights(ref mut s) = self.view { s.apply_route(r); }
            }
            Route::Deploy(ref r) => {
                self.start_deploy_view();
                if let ActiveView::Deploy(ref mut s) = self.view { s.apply_route(r); }
            }
            Route::ExternalMatches(ref r) => {
                self.start_external_matches_view();
                if let ActiveView::ExternalMatches(ref mut s) = self.view { s.apply_route(r); }
            }
            Route::Config(_) => self.start_config_editor(),
            Route::Search(_) => self.start_tag_search(),
            Route::Files(_) => self.start_corpus_browser(),
            Route::Transaction(_) => self.start_tabbed_transaction_review(),
            // Overlay routes are not yet navigable via navigate_to
            _ => {}
        }
        self.sync_route();
    }

    /// Synchronize `current_route` from the active view's state.
    fn sync_route(&mut self) {
        self.current_route = self.view.to_route();
    }

    /// Start the tabbed transaction review lateral view.
    pub(crate) fn start_tabbed_transaction_review(&mut self) {
        self.last_lateral_view = widgets::LateralView::Transaction;
        let mut state = tabbed_transaction_review::TabbedTransactionReviewState::new();
        let details = self.transaction_decision_details().unwrap_or_default();
        state.review.refresh_decisions_from_details(details);
        self.view = ActiveView::TabbedTransactionReview(state);
        self.sync_route();
    }

    /// Start the config editor view.
    pub(crate) fn start_config_editor(&mut self) {
        self.last_lateral_view = widgets::LateralView::Config;
        let config = self.config();
        let kdl_content = self.query(mm_meta::protocol::ConfigKdlQuery);
        let kdl_content = if kdl_content.is_empty() { None } else { Some(kdl_content) };
        self.view =
            ActiveView::ConfigEditor(config_editor::ConfigEditorState::new(&config, kdl_content));
        self.sync_route();
    }

    /// Start the deploy lateral view.
    ///
    /// If there's work to do (deploy signals present), loads full deploy data
    /// and shows the preview. Otherwise shows the "up to date" modal with
    /// per-library file counts.
    pub(crate) fn start_deploy_view(&mut self) {
        self.last_lateral_view = widgets::LateralView::Deploy;
        let deploy_status = self.query(mm_meta::domain_queries::GetDeployStatus);

        if deploy_status.needs_action {
            let cached_data = self.query(mm_meta::domain_queries::GetDeployData);
            self.view = ActiveView::Deploy(deploy_modal::DeployViewState::preview(cached_data));
        } else {
            self.view = ActiveView::Deploy(deploy_modal::DeployViewState::up_to_date(
                deploy_status.library_file_counts,
            ));
        }
        self.sync_route();
    }

    pub(crate) fn start_corpus_browser(&mut self) {
        self.last_lateral_view = widgets::LateralView::Files;
        let config = self.config();
        let corpus_dir = config.storage_root.clone();
        let corpus_dir_rel = "corpus".to_string();

        // Compute relative paths for marker lookups
        let deploy_source_dirs: Vec<String> = config
            .source_dirs
            .iter()
            .map(|sd| format!("corpus/{}", sd.path.display()))
            .collect();
        let primary_zone_dirs = vec![
            "corpus".to_string(),
            "stash".to_string(),
        ];

        let mut browser_state = tree_browser::TreeBrowserState::corpus_browser(
            corpus_dir,
            corpus_dir_rel,
            deploy_source_dirs,
            primary_zone_dirs,
        );

        // Load initial directory listing via protocol
        let root_entries = self.query(mm_meta::domain_queries::GetDirectoryListing {
            zone: mm_meta::db_types::Zone::Corpus,
            parent: None,
        });
        browser_state.browser_mut().populate_root(root_entries);

        // Load packing marker data
        let packing = self.query(mm_meta::domain_queries::GetPackingDirs);
        let tree_browser::BrowserVariant::CorpusBrowser(ref mut v) = browser_state.variant_mut();
        let file_paths: std::collections::HashSet<String> = packing.file_paths.iter()
            .filter_map(|p| p.strip_prefix(&config.storage_root).ok())
            .map(|p| p.to_string_lossy().to_string())
            .collect();
        let dir_categories: std::collections::HashMap<String, mm_meta::signals::packing_category::PackingCategory> = packing.dir_categories.iter()
            .filter_map(|(p, &cat)| {
                p.strip_prefix(&config.storage_root).ok().map(|rel| (rel.to_string_lossy().to_string(), cat))
            })
            .collect();
        v.set_packing_markers(file_paths, dir_categories);

        self.view = ActiveView::CorpusBrowser(browser_state);
        self.sync_route();
    }

    // =========================================================================
    // Startup Flow
    // =========================================================================

    /// Complete startup: open persistent txn and transition to the
    /// appropriate view based on Witch state.
    pub(crate) fn complete_startup(&mut self) {
        // If leave_transactions_open is enabled, open a persistent transaction at startup
        if self.open_txn_mode() {
            let _ = self.start_transaction("Open");
        }

        // Pick the right view based on current Witch state:
        // - Full reasoning + idle → go straight to the default view
        // - Full reasoning + busy → show content analysis progress
        // - Not yet Full → show eyeballing progress
        let status = self.witch_status_fetch();
        match status.reasoning_level {
            mm_meta::witch_types::ReasoningLevel::Full if !status.has_pending => {
                self.start_default_view();
            }
            mm_meta::witch_types::ReasoningLevel::Full => {
                self.view = ActiveView::Progress {
                    screen: progress_screen::ProgressScreen::new_content_analysis(),
                    eye: eye::Eye::default(),
                };
            }
            _ => {
                self.view = ActiveView::Progress {
                    screen: progress_screen::ProgressScreen::new_eyeballing(),
                    eye: eye::Eye::default(),
                };
            }
        }
    }

    // =========================================================================
    // Session Expiry → Login Screen
    // =========================================================================

    /// Transition to the in-app login screen.
    pub(crate) fn enter_login_screen(&mut self, message: Option<&str>) {
        self.session_token = None;
        self.session_expired = false;
        self.view_stack.clear();
        self.view = ActiveView::LoginScreen(LoginScreenState::new(message.map(String::from)));
    }

    /// Send an unauthenticated wire request (for login from within the app).
    fn send_unauthenticated(
        &mut self,
        body: mm_meta::protocol::UnauthenticatedBody,
    ) -> Result<mm_meta::protocol::UnauthenticatedResponse, mm_meta::protocol::ProtocolError> {
        let req = mm_meta::wire::WireRequest::Unauthenticated {
            request_id: 0,
            body,
        };
        let resp = self.rpc.send_blocking(req)
            .map_err(|e| mm_meta::protocol::ProtocolError::Internal(e.to_string()))?;
        match resp {
            mm_meta::wire::WireResponse::Unauthenticated { result, .. } => result,
            _ => Err(mm_meta::protocol::ProtocolError::Internal(
                "unexpected wire response type".to_string(),
            )),
        }
    }

    /// Attempt re-login using credentials from the LoginScreen view state.
    pub(crate) fn attempt_relogin(&mut self) {
        let (username, password) = if let ActiveView::LoginScreen(ref s) = self.view {
            (s.username.value().trim().to_string(), s.password.value().to_string())
        } else {
            return;
        };

        if username.is_empty() {
            if let ActiveView::LoginScreen(ref mut s) = self.view {
                s.error_message = Some("Username required".to_string());
            }
            return;
        }

        use mm_meta::protocol::{AuthResponse, UnauthenticatedBody, UnauthenticatedResponse};
        let result = self.send_unauthenticated(UnauthenticatedBody::Login {
            username,
            password,
        });

        match result {
            Ok(UnauthenticatedResponse::Auth(AuthResponse::Token(token))) => {
                self.session_token = Some(token);
                self.complete_startup();
            }
            Ok(UnauthenticatedResponse::Auth(AuthResponse::Failed(msg))) => {
                if let ActiveView::LoginScreen(ref mut s) = self.view {
                    s.error_message = Some(msg);
                    s.password.clear();
                }
            }
            Ok(_) => {
                if let ActiveView::LoginScreen(ref mut s) = self.view {
                    s.error_message = Some("Unexpected response".to_string());
                }
            }
            Err(e) => {
                if let ActiveView::LoginScreen(ref mut s) = self.view {
                    s.error_message = Some(format!("{e}"));
                }
            }
        }
    }

}

// ============================================================================
// Wire Protocol Helpers
// ============================================================================

impl App {
    fn send_authenticated(
        &mut self,
        body: mm_meta::protocol::AuthenticatedBody,
    ) -> Result<mm_meta::protocol::AuthenticatedResponse, mm_meta::protocol::ProtocolError> {
        let token = self
            .session_token
            .clone()
            .expect("send_authenticated called before login");
        let req = mm_meta::wire::WireRequest::Authenticated {
            request_id: 0,
            token,
            body: Box::new(body),
        };
        let resp = self.rpc.send_blocking(req)
            .map_err(|e| mm_meta::protocol::ProtocolError::Internal(e.to_string()))?;
        let result = match resp {
            mm_meta::wire::WireResponse::Authenticated { result, .. } => *result,
            _ => Err(mm_meta::protocol::ProtocolError::Internal(
                "unexpected wire response type".to_string(),
            )),
        };
        if matches!(&result, Err(mm_meta::protocol::ProtocolError::InvalidSession)) {
            self.session_expired = true;
        }
        result
    }

    pub(crate) fn query<Q: mm_meta::protocol::ProtocolQuery>(&mut self, q: Q) -> Q::Response {
        let body = mm_meta::protocol::AuthenticatedBody::Query(q.into_payload());
        match self.send_authenticated(body) {
            Ok(mm_meta::protocol::AuthenticatedResponse::Query(qr)) => Q::extract_response(*qr),
            Ok(_) => unreachable!("protocol bug: wrong response area"),
            Err(e) => panic!("protocol query failed: {e}"),
        }
    }

    pub(crate) fn send_transaction(
        &mut self,
        payload: mm_meta::protocol::TransactionPayload,
    ) -> mm_meta::protocol::TransactionResponse {
        let body = mm_meta::protocol::AuthenticatedBody::Transaction(payload);
        match self.send_authenticated(body) {
            Ok(mm_meta::protocol::AuthenticatedResponse::Transaction(tr)) => tr,
            Ok(_) => unreachable!("protocol bug: wrong response area"),
            Err(_) => mm_meta::protocol::TransactionResponse::Error(
                mm_meta::decisions::TransactionError::NotAcceptingMutations,
            ),
        }
    }

    pub(crate) fn send_command(
        &mut self,
        payload: mm_meta::protocol::CommandPayload,
    ) -> Result<mm_meta::protocol::CommandResponse, mm_meta::protocol::ProtocolError> {
        let body = mm_meta::protocol::AuthenticatedBody::Command(Box::new(payload));
        match self.send_authenticated(body)? {
            mm_meta::protocol::AuthenticatedResponse::Command(cr) => Ok(cr),
            _ => unreachable!("protocol bug: wrong response area"),
        }
    }

    pub(crate) fn witch_status_fetch(&mut self) -> WitchStatus {
        self.query(mm_meta::protocol::StatusQuery)
    }

    pub(crate) fn invalidate_config_cache(&mut self) {
        self.cached_config = None;
    }

    pub(crate) fn start_transaction(&mut self, label: &str) -> Result<(), mm_meta::protocol::ProtocolError> {
        match self.send_transaction(mm_meta::protocol::TransactionPayload::Start {
            label: label.to_owned(),
        }) {
            mm_meta::protocol::TransactionResponse::Ok => Ok(()),
            mm_meta::protocol::TransactionResponse::Error(e) => Err(mm_meta::protocol::ProtocolError::Transaction(e)),
            _ => unreachable!("protocol bug: wrong transaction response"),
        }
    }

    pub(crate) fn add_decision(
        &mut self,
        key: mm_meta::decisions::DecisionKey,
        decision: mm_meta::decisions::Decision,
    ) -> Result<(), mm_meta::protocol::ProtocolError> {
        match self.send_transaction(mm_meta::protocol::TransactionPayload::AddDecision { key, decision }) {
            mm_meta::protocol::TransactionResponse::Ok => Ok(()),
            mm_meta::protocol::TransactionResponse::Error(e) => Err(mm_meta::protocol::ProtocolError::Transaction(e)),
            _ => unreachable!("protocol bug: wrong transaction response"),
        }
    }

    pub(crate) fn remove_decision(&mut self, key: &mm_meta::decisions::DecisionKey) -> Result<(), mm_meta::protocol::ProtocolError> {
        match self.send_transaction(mm_meta::protocol::TransactionPayload::RemoveDecision { key: key.clone() }) {
            mm_meta::protocol::TransactionResponse::Ok => Ok(()),
            mm_meta::protocol::TransactionResponse::Error(e) => Err(mm_meta::protocol::ProtocolError::Transaction(e)),
            _ => unreachable!("protocol bug: wrong transaction response"),
        }
    }

    pub(crate) fn confirm_transaction(&mut self) -> Result<(), mm_meta::protocol::ProtocolError> {
        match self.send_transaction(mm_meta::protocol::TransactionPayload::Confirm) {
            mm_meta::protocol::TransactionResponse::Ok => Ok(()),
            mm_meta::protocol::TransactionResponse::Error(e) => Err(mm_meta::protocol::ProtocolError::Transaction(e)),
            _ => unreachable!("protocol bug: wrong transaction response"),
        }
    }

    pub(crate) fn discard_transaction(&mut self) -> Result<mm_meta::decisions::DiscardSummary, mm_meta::protocol::ProtocolError> {
        match self.send_transaction(mm_meta::protocol::TransactionPayload::Discard) {
            mm_meta::protocol::TransactionResponse::Discarded(s) => Ok(s),
            mm_meta::protocol::TransactionResponse::Error(e) => Err(mm_meta::protocol::ProtocolError::Transaction(e)),
            _ => unreachable!("protocol bug: wrong transaction response"),
        }
    }

    pub(crate) fn transaction_decision_details(&mut self) -> Result<Vec<mm_meta::protocol::DecisionDetail>, mm_meta::protocol::ProtocolError> {
        match self.send_transaction(mm_meta::protocol::TransactionPayload::GetDetails) {
            mm_meta::protocol::TransactionResponse::Details(d) => Ok(d),
            mm_meta::protocol::TransactionResponse::Error(e) => Err(mm_meta::protocol::ProtocolError::Transaction(e)),
            _ => unreachable!("protocol bug: wrong transaction response"),
        }
    }

    /// Server-side bulk approval. Single round-trip: server loads review +
    /// staging data, builds decisions, and stages them into the pending
    /// transaction (opens one if none is active, else appends so prior
    /// approvals aren't clobbered). Returns the full `ApprovalSummary`.
    pub(crate) fn batch_approve_releases(
        &mut self,
        release_ids: Vec<String>,
    ) -> Result<mm_meta::external::approval::ApprovalSummary, mm_meta::protocol::ProtocolError>
    {
        match self.send_transaction(
            mm_meta::protocol::TransactionPayload::BatchApproveReleases { release_ids },
        ) {
            mm_meta::protocol::TransactionResponse::BatchApprovalStaged(summary) => Ok(summary),
            mm_meta::protocol::TransactionResponse::Error(e) => {
                Err(mm_meta::protocol::ProtocolError::Transaction(e))
            }
            _ => unreachable!("protocol bug: wrong transaction response"),
        }
    }

    /// Server-side bulk VA-override apply. Mirrors `batch_approve_releases`
    /// but for `BatchApplyVaOverrides`. Returns the `VaOverrideSummary`.
    pub(crate) fn batch_apply_va_overrides(
        &mut self,
        applications: Vec<mm_meta::protocol::VaOverrideApplication>,
    ) -> Result<mm_meta::external::va_override::VaOverrideSummary, mm_meta::protocol::ProtocolError>
    {
        match self.send_transaction(
            mm_meta::protocol::TransactionPayload::BatchApplyVaOverrides { applications },
        ) {
            mm_meta::protocol::TransactionResponse::VaOverridesStaged(summary) => Ok(summary),
            mm_meta::protocol::TransactionResponse::Error(e) => {
                Err(mm_meta::protocol::ProtocolError::Transaction(e))
            }
            _ => unreachable!("protocol bug: wrong transaction response"),
        }
    }

    /// Queue a background task. Returns `Ok(None)` on success, `Ok(Some(reason))` if
    /// the server rejected the command, or `Err` on protocol failure.
    pub(crate) fn queue_task(&mut self, task: mm_meta::protocol::BackgroundTask) -> Result<Option<String>, mm_meta::protocol::ProtocolError> {
        match self.send_command(mm_meta::protocol::CommandPayload::QueueTask(task))? {
            mm_meta::protocol::CommandResponse::Ok => Ok(None),
            mm_meta::protocol::CommandResponse::Failed(reason) => Ok(Some(reason)),
            mm_meta::protocol::CommandResponse::Goodbye => Err(mm_meta::protocol::ProtocolError::Internal(
                "unexpected Goodbye for QueueTask".to_string(),
            )),
        }
    }

    pub(crate) fn shutdown(&mut self) -> Result<(), mm_meta::protocol::ProtocolError> {
        match self.send_command(mm_meta::protocol::CommandPayload::Shutdown)? {
            mm_meta::protocol::CommandResponse::Goodbye => Ok(()),
            mm_meta::protocol::CommandResponse::Ok
            | mm_meta::protocol::CommandResponse::Failed(_) => Err(mm_meta::protocol::ProtocolError::Internal(
                "expected Goodbye, got Ok".to_string(),
            )),
        }
    }

}

// ============================================================================
// Pre-Auth Socket Wrapper (used during startup before login)
// ============================================================================

pub(crate) mod startup_socket {
    /// Pre-auth socket wrapper for startup operations (setup, login).
    pub struct StartupSocket<'a> {
        pub(crate) socket: &'a mut std::os::unix::net::UnixStream,
        next_id: u64,
    }

    impl<'a> StartupSocket<'a> {
        pub fn new(socket: &'a mut std::os::unix::net::UnixStream) -> Self {
            Self { socket, next_id: 0 }
        }

        fn send_unauthenticated(
            &mut self,
            body: mm_meta::protocol::UnauthenticatedBody,
        ) -> Result<mm_meta::protocol::UnauthenticatedResponse, mm_meta::protocol::ProtocolError> {
            let id = self.next_id;
            self.next_id += 1;
            let req = mm_meta::wire::WireRequest::Unauthenticated {
                request_id: id,
                body,
            };
            mm_meta::wire::write_frame(self.socket, &req)
                .map_err(|e| mm_meta::protocol::ProtocolError::Internal(format!("socket write: {e}")))?;
            // The server may push WitchEvent frames at any time. Skip them
            // until we receive the actual response to our request.
            loop {
                let resp: mm_meta::wire::WireResponse = mm_meta::wire::read_frame(self.socket)
                    .map_err(|e| mm_meta::protocol::ProtocolError::Internal(format!("socket read: {e}")))?;
                match resp {
                    mm_meta::wire::WireResponse::Unauthenticated { result, .. } => return result,
                    mm_meta::wire::WireResponse::Event(_) => continue,
                    _ => return Err(mm_meta::protocol::ProtocolError::Internal(
                        "unexpected wire response type".to_string(),
                    )),
                }
            }
        }

        /// Query setup status from the Witch.
        ///
        /// Returns `(needs_setup, suggested_root)`. Two states: either setup
        /// is needed (fresh install) or the system is ready for login.
        pub fn setup_status(&mut self) -> (bool, Option<std::path::PathBuf>) {
            match self.send_unauthenticated(mm_meta::protocol::UnauthenticatedBody::SetupQuery) {
                Ok(mm_meta::protocol::UnauthenticatedResponse::SetupStatus { needs_setup, suggested_root }) => {
                    (needs_setup, suggested_root)
                }
                _ => (false, None),
            }
        }

        pub fn complete_setup(
            &mut self,
            root: std::path::PathBuf,
            first_user: Option<(String, String)>,
        ) -> Result<(), mm_meta::protocol::ProtocolError> {
            match self.send_unauthenticated(
                mm_meta::protocol::UnauthenticatedBody::CompleteSetup { root, first_user },
            )? {
                mm_meta::protocol::UnauthenticatedResponse::SetupComplete => Ok(()),
                _ => Err(mm_meta::protocol::ProtocolError::Internal(
                    "unexpected response to setup".to_string(),
                )),
            }
        }

        pub fn notify_db_ready(&mut self) {
            let id = self.next_id;
            self.next_id += 1;
            let _ = mm_meta::wire::write_frame(
                self.socket,
                &mm_meta::wire::WireRequest::NotifyDbReady { request_id: id },
            );
            // Drain until we get the Ack, skipping any pushed events.
            loop {
                match mm_meta::wire::read_frame::<_, mm_meta::wire::WireResponse>(self.socket) {
                    Ok(mm_meta::wire::WireResponse::Event(_)) => continue,
                    _ => break,
                }
            }
        }

        pub fn login(
            &mut self,
            username: &str,
            password: &str,
        ) -> Result<mm_meta::auth::SessionToken, String> {
            use mm_meta::protocol::{AuthResponse, UnauthenticatedBody, UnauthenticatedResponse};
            let response = self.send_unauthenticated(UnauthenticatedBody::Login {
                username: username.to_string(),
                password: password.to_string(),
            });
            match response {
                Ok(UnauthenticatedResponse::Auth(AuthResponse::Token(token))) => Ok(token),
                Ok(UnauthenticatedResponse::Auth(AuthResponse::Failed(msg))) => Err(msg),
                Ok(_) => Err("unexpected response to login".to_string()),
                Err(e) => Err(format!("{e}")),
            }
        }
    }
}

// ============================================================================
// Rendering
// ============================================================================

fn render(f: &mut Frame, app: &mut App) {
    // Build status bar lines
    let status_line_1 = if let Some(ref msg) = app.status_message {
        Some(msg.clone())
    } else {
        app.view.selected_path().map(|s| s.to_string())
    };

    let status_line_2 = {
        app.witch_status().transaction.as_ref().map(|t| {
            let dec = t.decision_count;
            let mut_ = t.mutation_count;
            let pd = if dec == 1 { "" } else { "s" };
            let pm = if mut_ == 1 { "" } else { "s" };
            format!("Transaction \"{}\": {} decision{}, {} mutation{} staged", t.label, dec, pd, mut_, pm)
        })
    };

    render::render_app(f, app, status_line_1, status_line_2);
}

// ============================================================================
// Entry Point
// ============================================================================

pub fn run_tui(
    mut socket: std::os::unix::net::UnixStream,
) -> Result<()> {
    mm_meta::logging::log_general("=== MM TUI startup ===");

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste
    )?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Pre-auth startup operations via socket
    let session_token;
    {
        let mut startup = StartupSocket::new(&mut socket);

        // Two states: needs setup (first time) or ready for login.
        let (needs_setup, suggested_root) = startup.setup_status();
        if needs_setup {
            // First-time setup: pick archive root, create first account
            let root = startup::run_directory_picker(&mut terminal, suggested_root)?;
            let first_user = startup::first_time_setup::run_create_account(&mut terminal)?;

            startup
                .complete_setup(root, Some(first_user))
                .map_err(|e| anyhow::anyhow!("{}", e))?;

            // Notify auth thread that DB is now available
            startup.notify_db_ready();
        }

        // Auth: login to obtain session token
        session_token = startup::login::run_login_screen(&mut terminal, &mut startup, None)?;
    }

    // Init the image picker (env-based detection, no stdin probing).
    let art_picker = widgets::AlbumArtPicker::init();

    // Hand socket to the async RPC thread (ownership transfer).
    let rpc = rpc::start_rpc_thread(socket);

    // Config + DB now guaranteed. App fetches config via protocol.
    let mut app = App::new(rpc, session_token, art_picker);

    // Check if the Witch is already Ready (no startup maintenance needed)
    // or if she's running maintenance (Reconciling/Vacuuming)
    {
        let status = app.witch_status_fetch();
        if status.startup_state == mm_meta::witch_types::WitchStartupState::Ready {
            app.startup_complete = true;
            app.complete_startup();
        } else {
            // Witch is in Reconciling or Vacuuming — show maintenance view
            app.view = ActiveView::StartupMaintenance;
        }
    }

    let res = run_app(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        DisableBracketedPaste
    )?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        eprintln!("Error: {:?}", err);
    }

    Ok(())
}

fn run_app<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> io::Result<()> {
    let signal_received = Arc::new(AtomicBool::new(false));

    if let Err(e) = register_signal_handlers(Arc::clone(&signal_received)) {
        mm_meta::logging::log_error(format!("Failed to register signal handlers: {}", e));
    }

    loop {
        // Drain pushed status events from the Witch (replaces polling)
        while let Some(event) = app.rpc.try_recv_event() {
            match event {
                mm_meta::witch_types::WitchEvent::StatusChanged(status) => {
                    app.cached_status = status;
                }
            }
        }

        if app.session_expired {
            app.enter_login_screen(Some("Session expired — please log in again"));
        }

        if signal_received.swap(false, Ordering::SeqCst) {
            app.handle_ctrl_c();
        }

        // Observe startup maintenance completion (Witch auto-runs reconciliation/vacuum)
        if !app.startup_complete {
            let state = app.witch_status().startup_state;
            if state == mm_meta::witch_types::WitchStartupState::Ready {
                app.startup_complete = true;
                app.complete_startup();
                terminal.draw(|f| render(f, app))?;
                continue;
            }
        }

        // (Witch ticks herself on the main thread — no manual tick needed)

        // Detect events via generation counter diffing against WitchStatus
        {
            let mut view_data_stale = false;

            // Mutations completed — views dependent on mutation state need refresh
            if app.cached_status.mutations_generation != app.prev_mutations_generation {
                app.prev_mutations_generation = app.cached_status.mutations_generation;
                view_data_stale = true;
            }

            // Computations completed — views dependent on signal-derived data need refresh
            if app.cached_status.computations_generation != app.prev_computations_generation {
                app.prev_computations_generation = app.cached_status.computations_generation;
                view_data_stale = true;
            }

            // New error → show status message
            if app.cached_status.error_generation != app.prev_error_generation {
                app.prev_error_generation = app.cached_status.error_generation;
                if let Some(ref err) = app.cached_status.last_error {
                    app.status_message = Some(format!("Task failed: {}", err));
                }
            }

            // Config updated — invalidate cache so next config() re-fetches
            if app.cached_status.config_generation != app.prev_config_generation {
                app.prev_config_generation = app.cached_status.config_generation;
                app.invalidate_config_cache();
                // Rebuild path resolver with new root
                let new_config = app.config();
                app.resolver = PathResolver::from_config(&new_config);
            }

            if view_data_stale && std::time::Instant::now() >= app.next_refresh_allowed_at {
                app.refresh_active_view_data();
                // Throttle: ensure at least 500ms between refreshes so the TUI can
                // render frames and process input between blocking queries. Without
                // this, rapid generation-counter bumps cause an infinite query loop
                // (each blocking query gives the Witch time to bump again → re-query).
                app.next_refresh_allowed_at = std::time::Instant::now()
                    + std::time::Duration::from_millis(500);
            }
        }

        // ExternalMatches: poll fetch status each tick when active (progress display)
        if matches!(app.view, ActiveView::ExternalMatches(_)) {
            let new_fetch_active = app.cached_status.is_external_fetch_active;
            let fetch_progress = app.cached_status.external_fetch_progress.clone();
            let new_cover_art_active = app.cached_status.is_cover_art_fetch_active;
            let cover_art_progress = app.cached_status.cover_art_progress.clone();
            let new_deezer_active = app.cached_status.is_deezer_fetch_active;
            let deezer_progress = app.cached_status.deezer_progress.clone();
            let mut fetch_changed = false;
            if let ActiveView::ExternalMatches(ref mut s) = app.view {
                fetch_changed = s.data.fetch_active != new_fetch_active
                    || s.data.cover_art_active != new_cover_art_active
                    || s.data.deezer_active != new_deezer_active;
                s.data.fetch_active = new_fetch_active;
                s.data.fetch_progress = fetch_progress;
                s.data.cover_art_active = new_cover_art_active;
                s.data.cover_art_progress = cover_art_progress;
                s.data.deezer_active = new_deezer_active;
                s.data.deezer_progress = deezer_progress;
                if s.data.fetch_active || s.data.cover_art_active || s.data.deezer_active {
                    s.interaction.tick_count = s.interaction.tick_count.wrapping_add(1);
                }
            }
            if fetch_changed {
                // Fetch state toggled — re-query data and rebuild items
                app.refresh_active_view_data();
            }
        }

        // Tick view-specific state machines
        if matches!(app.view, ActiveView::Progress { .. }) {
            app.tick_progress_screen();
        }
        if matches!(app.view, ActiveView::ProgressiveWork(_)) {
            app.tick_progressive_worker();
        }

        terminal.draw(|f| render(f, app))?;

        // Tick tag search for pending bulk edit (after modal has rendered)
        app.tick_tag_search();

        if event::poll(std::time::Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) => {
                    if key.code == crossterm::event::KeyCode::Char('c')
                        && key
                            .modifiers
                            .contains(crossterm::event::KeyModifiers::CONTROL)
                    {
                        app.handle_ctrl_c();
                    } else {
                        app.handle_input(input::map_key(key));
                    }
                }
                Event::Mouse(mouse) => match mouse.kind {
                    MouseEventKind::ScrollUp => {
                        app.handle_input(InputAction::NavUp);
                    }
                    MouseEventKind::ScrollDown => {
                        app.handle_input(InputAction::NavDown);
                    }
                    MouseEventKind::Down(MouseButton::Left) => {
                        app.handle_click(mouse.column, mouse.row);
                    }
                    _ => {}
                },
                Event::Paste(text) => {
                    app.handle_input(InputAction::Paste(text));
                }
                _ => {}
            }
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}

fn register_signal_handlers(flag: Arc<AtomicBool>) -> Result<(), Box<dyn std::error::Error>> {
    use signal_hook::consts::{SIGINT, SIGTERM};
    use signal_hook::flag;

    flag::register(SIGINT, Arc::clone(&flag))?;
    flag::register(SIGTERM, flag)?;

    Ok(())
}
