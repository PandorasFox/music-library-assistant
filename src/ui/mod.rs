//! Terminal User Interface
//!
//! ## Module Organization
//! - `types.rs` - UiMode, traits, modal state types
//! - `action_handlers.rs` - View-specific action processing
//! - `tag_editor_ops.rs` - Tag editor launching and navigation
//! - `tick.rs` - Per-frame update logic for each view
//!
//! ## Adding New Views
//! 1. Add variant to `UiMode` in `types.rs`
//! 2. Add action handler in `action_handlers.rs`
//! 3. Add tick handler in `tick.rs` if needed
//! 4. Add render case in `render.rs`
//!
//! ## Existing submodules (views)
//! - `insights_view/` - Main insights dashboard
//! - `tag_editor/` - Unified tag editing
//! - `tag_search/` - Tag search interface
//! - `tree_browser/` - File tree navigation
//! - `deploy_flow/` - Deployment preview

mod action_handlers;
mod tag_editor_ops;
mod tick;
mod types;

pub mod operator_decisions;
pub mod transaction_review;

pub mod app;
pub mod bulk_selection;
pub mod compound_split_v2;
pub mod corrupt_file_flow;
pub mod deploy_flow;
pub mod directory_cluster_flow;
pub mod eye;
pub mod filter_popup;
pub mod helpers;
pub mod insights_view;
pub mod missing_file_flow;
pub mod missing_directory_flow;
pub mod moved_file_flow;
pub mod oob_conflict_flow;
pub mod oob_sync_flow;
pub mod progress_screen;
pub mod render;
pub mod shit_format_flow;
pub mod startup;
pub mod subpar_duplicate_flow;
pub mod tag_canonicity_v2;
pub mod tag_editor;
pub mod tag_search;
pub mod tree_browser;
pub mod wait_state;
pub mod widgets;
pub mod progressive_worker;

// Re-export types for convenience
pub(crate) use types::{UiMode, ExitConfirmModalState};

/// Context for filter popup - determines where to apply filter results.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterPopupContext {
    /// Filter for corpus browser tree
    CorpusBrowser,
    /// Filter for OOB sync resolution flow
    OobSync,
    /// Filter for OOB conflict resolution flow
    OobConflict,
}

use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, MouseButton, MouseEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    Frame, Terminal,
};
use std::collections::VecDeque;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use crate::config::Config;
use app::EyeAnimation;
use types::ProgressStatsUpdater;

// ============================================================================
// Tag Canonicity Navigation
// ============================================================================

/// Tracks the list of signals for Tab/Shift-Tab navigation in tag canonicity modal.
#[derive(Debug, Clone)]
pub(super) struct TagCanonicityClusters {
    /// Signal IDs in navigation order
    pub signal_ids: Vec<i64>,
    /// Current index into signal_ids
    pub current_index: usize,
}

impl TagCanonicityClusters {
    pub fn new(signal_ids: Vec<i64>) -> Self {
        Self { signal_ids, current_index: 0 }
    }

    pub fn current_signal_id(&self) -> Option<i64> {
        self.signal_ids.get(self.current_index).copied()
    }

    pub fn next(&mut self) -> bool {
        if self.current_index + 1 < self.signal_ids.len() {
            self.current_index += 1;
            true
        } else {
            false
        }
    }

    pub fn prev(&mut self) -> bool {
        if self.current_index > 0 {
            self.current_index -= 1;
            true
        } else {
            false
        }
    }

    pub fn is_last(&self) -> bool {
        self.current_index + 1 >= self.signal_ids.len()
    }
}

// ============================================================================
// Application State
// ============================================================================

/// Main application state
pub(crate) struct App {
    pub(super) config: Config,
    should_quit: bool,
    pub(super) status_message: Option<String>,

    // UI mode and state
    pub(super) mode: UiMode,
    pub(super) tree_browser: Option<tree_browser::TreeBrowserState>,
    pub(super) deployment_preview: Option<deploy_flow::DeploymentPreviewState>,
    // Missing file resolution modal
    pub(super) missing_file_preview: Option<missing_file_flow::MissingFilePreviewState>,
    // Missing directory acknowledgment modal
    pub(super) missing_directory_preview: Option<missing_directory_flow::MissingDirectoryPreviewState>,
    // Tag canonicity resolution three-pane modal
    pub(super) tag_canonicity_state: Option<tag_canonicity_v2::TagCanonicalityStateV2>,
    // Tag canonicity cluster navigation (signal IDs and current index)
    pub(super) tag_canonicity_clusters: Option<TagCanonicityClusters>,
    // Compound tag split modal (v2 three-pane)
    pub(super) compound_split_state: Option<compound_split_v2::CompoundSplitStateV2>,
    // Compound split cluster navigation (signal IDs and current index)
    pub(super) compound_split_clusters: Option<compound_split_v2::CompoundSplitClustersV2>,
    // Whether we're in safe mode (bulk) or review mode
    pub(super) compound_split_safe_mode: bool,
    // OOB tag sync resolution
    pub(super) oob_sync_state: Option<oob_sync_flow::OobSyncState>,
    // OOB tag conflict inspection
    pub(super) oob_conflict_state: Option<oob_conflict_flow::OobConflictState>,
    // Moved file acknowledgement
    pub(super) moved_file_state: Option<moved_file_flow::MovedFileState>,
    // Standardized transaction review modal
    pub(super) transaction_review: Option<transaction_review::TransactionReviewState>,
    // Unified tag editor (transaction-based)
    pub(super) unified_tag_editor: Option<tag_editor::UnifiedTagEditorState>,
    // Exit confirmation modal
    pub(super) exit_confirm_modal_state: Option<ExitConfirmModalState>,
    // Unified progress screen (startup eyeballing, content analysis, signal refresh)
    pub(super) progress_screen: Option<progress_screen::ProgressScreen>,
    // Insights view (lateral view ring)
    pub(super) insights_view: Option<insights_view::InsightsViewState>,
    // Tag search (lateral view ring)
    pub(super) tag_search: Option<tag_search::TagSearchState>,
    // Intake confirmation modal
    pub(super) intake_confirmation: Option<startup::IntakeConfirmationState>,
    // Corrupt file resolution modal
    pub(super) corrupt_file_preview: Option<corrupt_file_flow::CorruptFilePreviewState>,
    // Shit format transcode resolution modal
    pub(super) shit_format_preview: Option<shit_format_flow::ShitFormatPreviewState>,
    // Subpar duplicate resolution modal
    pub(super) subpar_duplicate_preview: Option<subpar_duplicate_flow::SubparDuplicatePreviewState>,
    // Directory overlap cluster resolution modal
    pub(super) directory_cluster_preview: Option<directory_cluster_flow::DirectoryClusterPreviewState>,
    // Progressive work modal (timed bulk operations with progress bar)
    pub(super) progressive_worker: Option<progressive_worker::ProgressiveWorkerState>,

    // The Witch - enforcer of orderliness, handles all mutations and background work
    pub(super) witch: Option<crate::witch::Witch>,

    // Log channel receiver, held until the Witch takes ownership
    log_rx: Option<std::sync::mpsc::Receiver<crate::logging::LogOp>>,

    // Throughput tracking for rolling average (timestamp, bytes_processed)
    _throughput_samples: VecDeque<(Instant, u64)>,

    // Eye animation
    eye: EyeAnimation,

    // Filter popup overlay (Ctrl+F in resolution flows and corpus browser)
    pub(super) filter_popup_state: Option<filter_popup::FilterPopupState>,
    // Context for filter popup (determines where to apply results)
    pub(super) filter_popup_context: Option<FilterPopupContext>,
}

impl App {
    /// Create a new App with a pre-existing Witch instance.
    ///
    /// Used when the Witch is created early (before migrations) and
    /// needs to be passed to the App rather than created lazily.
    fn new_with_witch(config: Config, witch: crate::witch::Witch) -> Self {
        Self {
            config,
            should_quit: false,
            status_message: None,
            mode: UiMode::Insights,
            tree_browser: None,
            deployment_preview: None,
            missing_file_preview: None,
            missing_directory_preview: None,
            tag_canonicity_state: None,
            tag_canonicity_clusters: None,
            compound_split_state: None,
            compound_split_clusters: None,
            compound_split_safe_mode: false,
            oob_sync_state: None,
            oob_conflict_state: None,
            moved_file_state: None,
            transaction_review: None,
            unified_tag_editor: None,
            exit_confirm_modal_state: None,
            progress_screen: None,
            insights_view: None,
            tag_search: None,
            intake_confirmation: None,
            corrupt_file_preview: None,
            shit_format_preview: None,
            subpar_duplicate_preview: None,
            directory_cluster_preview: None,
            progressive_worker: None,
            witch: Some(witch),
            log_rx: None,  // Already consumed by Witch
            _throughput_samples: VecDeque::with_capacity(100),
            eye: EyeAnimation::default(),
            filter_popup_state: None,
            filter_popup_context: None,
        }
    }

    fn handle_key(&mut self, key: crossterm::event::KeyEvent) {
        // Filter popup intercepts keys when active
        if let Some(ref mut popup) = self.filter_popup_state {
            let action = popup.handle_key(key);
            match action {
                filter_popup::FilterPopupAction::None => return,
                filter_popup::FilterPopupAction::Apply => {
                    // Apply filter condition based on context
                    let condition = popup.condition.clone();
                    match self.filter_popup_context {
                        Some(FilterPopupContext::CorpusBrowser) => {
                            // Apply to corpus browser tree
                            if let Some(ref mut browser) = self.tree_browser {
                                if let Some(ref mut witch) = self.witch {
                                    let read_db = witch.read_db();
                                    browser.apply_filter(condition, &read_db);
                                }
                            }
                        }
                        Some(FilterPopupContext::OobSync) => {
                            if let Some(ref mut state) = self.oob_sync_state {
                                state.apply_filter(condition);
                            }
                        }
                        Some(FilterPopupContext::OobConflict) => {
                            if let Some(ref mut state) = self.oob_conflict_state {
                                state.apply_filter(condition);
                            }
                        }
                        None => {
                            // Legacy fallback: try OOB flows
                            if let Some(ref mut state) = self.oob_sync_state {
                                state.apply_filter(condition);
                            } else if let Some(ref mut state) = self.oob_conflict_state {
                                state.apply_filter(condition);
                            }
                        }
                    }
                    self.filter_popup_state = None;
                    self.filter_popup_context = None;
                    return;
                }
                filter_popup::FilterPopupAction::Clear => {
                    // Clear filter based on context
                    match self.filter_popup_context {
                        Some(FilterPopupContext::CorpusBrowser) => {
                            if let Some(ref mut browser) = self.tree_browser {
                                browser.clear_filter();
                            }
                        }
                        Some(FilterPopupContext::OobSync) => {
                            if let Some(ref mut state) = self.oob_sync_state {
                                state.clear_filter();
                            }
                        }
                        Some(FilterPopupContext::OobConflict) => {
                            if let Some(ref mut state) = self.oob_conflict_state {
                                state.clear_filter();
                            }
                        }
                        None => {
                            // Legacy fallback
                            if let Some(ref mut state) = self.oob_sync_state {
                                state.clear_filter();
                            } else if let Some(ref mut state) = self.oob_conflict_state {
                                state.clear_filter();
                            }
                        }
                    }
                    self.filter_popup_state = None;
                    self.filter_popup_context = None;
                    return;
                }
                filter_popup::FilterPopupAction::Cancel => {
                    // Just close popup
                    self.filter_popup_state = None;
                    self.filter_popup_context = None;
                    return;
                }
            }
        }

        match self.mode {
            UiMode::Progress => {
                // Progress screen ignores keys - can't interact during loading/computation
            }
            UiMode::DeploymentPreview => {
                if let Some(ref mut preview) = self.deployment_preview {
                    let action = preview.handle_key(key);
                    self.handle_deployment_preview_action(action);
                }
            }
            UiMode::ExitConfirmModal => {
                if let Some(ref mut state) = self.exit_confirm_modal_state {
                    match key.code {
                        KeyCode::Left | KeyCode::Right | KeyCode::Tab => {
                            // Toggle between Yes and No
                            state.selected_no = !state.selected_no;
                        }
                        KeyCode::Enter | KeyCode::Char(' ') => {
                            if state.selected_no {
                                // "No" selected - return to Insights view
                                self.exit_confirm_modal_state = None;
                                self.start_insights_view();
                            } else {
                                // "Yes" selected - actually quit
                                self.should_quit = true;
                            }
                        }
                        KeyCode::Esc => {
                            // Esc returns to Insights view
                            self.exit_confirm_modal_state = None;
                            self.start_insights_view();
                        }
                        KeyCode::Char('y') | KeyCode::Char('Y') => {
                            // 'y' confirms exit
                            self.should_quit = true;
                        }
                        KeyCode::Char('n') | KeyCode::Char('N') => {
                            // 'n' cancels - return to Insights view
                            self.exit_confirm_modal_state = None;
                            self.start_insights_view();
                        }
                        _ => {}
                    }
                }
            }
            UiMode::CorpusBrowser => {
                if let Some(ref mut browser) = self.tree_browser {
                    let action = browser.handle_key(key);
                    self.handle_tree_browser_action(action);
                }
            }
            UiMode::Insights => {
                if let Some(ref mut view) = self.insights_view {
                    let action = view.handle_key(key);
                    self.handle_insights_action(action);
                }
            }
            UiMode::IntakeConfirmation => {
                if let Some(ref mut state) = self.intake_confirmation {
                    // Get terminal size to compute visible height for scrolling
                    let visible_height = crossterm::terminal::size()
                        .map(|(_, h)| startup::intake_confirmation::compute_list_visible_height(
                            ratatui::layout::Rect::new(0, 0, 80, h)
                        ))
                        .unwrap_or(10);
                    let action = state.handle_key(key, visible_height);
                    self.handle_intake_confirmation_action(action);
                }
            }
            UiMode::UnifiedTagEditor => {
                if let Some(ref mut editor) = self.unified_tag_editor {
                    let action = editor.handle_key(key);
                    self.handle_unified_tag_editor_action(action);
                }
            }
            UiMode::TagSearch => {
                if let Some(ref mut search) = self.tag_search {
                    let action = search.handle_key(key);
                    self.handle_tag_search_action(action);
                }
            }
            UiMode::MissingFileResolution => {
                if let Some(ref mut preview) = self.missing_file_preview {
                    let action = preview.handle_key(key);
                    self.handle_missing_file_preview_action(action);
                }
            }
            UiMode::MissingDirectoryResolution => {
                if let Some(ref mut preview) = self.missing_directory_preview {
                    let action = preview.handle_key(key);
                    self.handle_missing_directory_preview_action(action);
                }
            }
            UiMode::TagCanonicityResolution => {
                if let Some(ref mut state) = self.tag_canonicity_state {
                    let action = state.handle_key(key);
                    self.handle_tag_canonicity_action(action);
                }
            }
            UiMode::TransactionReview => {
                if let Some(ref mut review) = self.transaction_review {
                    let action = review.handle_key(key);
                    self.handle_transaction_review_action(action);
                }
            }
            UiMode::CompoundTagSplit => {
                if let Some(ref mut state) = self.compound_split_state {
                    let action = state.handle_key(key);
                    self.handle_compound_split_action(action);
                }
            }
            UiMode::OobSyncResolution => {
                if let Some(ref mut state) = self.oob_sync_state {
                    let action = state.handle_key(key);
                    self.handle_oob_sync_action(action);
                }
            }
            UiMode::OobConflictInspection => {
                if let Some(ref mut state) = self.oob_conflict_state {
                    let action = state.handle_key(key);
                    self.handle_oob_conflict_action(action);
                }
            }
            UiMode::MovedFileAcknowledge => {
                if let Some(ref mut state) = self.moved_file_state {
                    let action = state.handle_key(key);
                    self.handle_moved_file_action(action);
                }
            }
            UiMode::CorruptFileResolution => {
                if let Some(ref mut preview) = self.corrupt_file_preview {
                    let action = preview.handle_key(key);
                    self.handle_corrupt_file_preview_action(action);
                }
            }
            UiMode::ShitFormatResolution => {
                if let Some(ref mut preview) = self.shit_format_preview {
                    let action = preview.handle_key(key);
                    self.handle_shit_format_preview_action(action);
                }
            }
            UiMode::SubparDuplicateResolution => {
                if let Some(ref mut preview) = self.subpar_duplicate_preview {
                    let action = preview.handle_key(key);
                    self.handle_subpar_duplicate_preview_action(action);
                }
            }
            UiMode::DirectoryClusterResolution => {
                if let Some(ref mut preview) = self.directory_cluster_preview {
                    let action = preview.handle_key(key);
                    self.handle_directory_cluster_preview_action(action);
                }
            }
            UiMode::ProgressiveWork => {
                // Progressive work modal ignores ALL keys - purely displays progress
                // Input is drained when work completes
            }
        }
    }


    /// Check if there are any pending operations (Witch work).
    pub(super) fn has_pending_operations(&self) -> bool {
        self.witch.as_ref().map(|d| d.has_pending()).unwrap_or(false)
    }

    /// Start the insights view.
    pub(super) fn start_insights_view(&mut self) {
        self.insights_view = Some(insights_view::InsightsViewState::new());
        self.mode = UiMode::Insights;
    }

    /// Stage a decision to the Witch's transaction and update editor state.
    ///
    /// Helper for StageDecision, StageDecisionAndNext, and StageDecisionAndReview actions.
    pub(super) fn stage_decision(&mut self, index: usize, mutations: Vec<crate::meta::mutations::Mutation>) {
        if let Some(the_witch) = self.witch.as_mut() {
            let label = self.unified_tag_editor
                .as_ref()
                .map(|e| e.current_item_label())
                .unwrap_or_else(|| "Tag edit".to_string());
            // Stage via sealed operator decision handler
            let _ = operator_decisions::stage_decision(the_witch, index, &label, mutations.clone());
        }
        // Track staged mutations for redundant confirmation skipping
        if let Some(ref mut editor) = self.unified_tag_editor {
            editor.set_staged_mutations(mutations);
        }
        self.status_message = Some(format!("Decision staged (item {})", index + 1));
    }

    /// Abort current operation and return to insights view with a status message.
    ///
    /// Helper for error handling in start_tag_editor_for_path.
    pub(super) fn abort_to_insights(&mut self, message: String) {
        self.status_message = Some(message);
        self.tree_browser = None;
        self.start_insights_view();
    }

    /// Update Witch stats on a progress state that implements the stats setter methods.
    ///
    /// Helper for updating queue depth, db stats, and worker stats from the Witch.
    pub(super) fn update_progress_stats<T>(&self, state: &mut T)
    where
        T: ProgressStatsUpdater,
    {
        if let Some(the_witch) = &self.witch {
            state.set_db_queue_depth(the_witch.db_queue_depth());
            state.set_db_stats(the_witch.db_stats());
            state.set_worker_stats(the_witch.worker_stats());
        }
    }

    pub(super) fn start_tag_search(&mut self) {
        self.tag_search = Some(tag_search::TagSearchState::new());
        self.mode = UiMode::TagSearch;
    }

    /// Start the lateral view identified by the given variant.
    /// Used by CycleNext/CyclePrev handlers to dispatch via LateralView::next()/prev().
    pub(super) fn start_lateral_view(&mut self, view: widgets::LateralView) {
        match view {
            widgets::LateralView::TagSearch => self.start_tag_search(),
            widgets::LateralView::CorpusBrowser => self.start_corpus_browser(),
            widgets::LateralView::Insights => self.start_insights_view(),
        }
    }

    pub(super) fn start_corpus_browser(&mut self) {
        let config = tree_browser::CorpusBrowserConfig::default();
        self.tree_browser = Some(tree_browser::TreeBrowserState::corpus_browser(
            self.config.corpus_dir(),
            config,
        ));
        self.mode = UiMode::CorpusBrowser;
    }

    /// Check Witch status and update UI with any failure messages.
    /// Note: Actual Witch ticking happens in run_app via witch().tick()
    fn check_witch_status(&mut self) {
        if let Some(ref the_witch) = self.witch {
            let status = the_witch.status();
            if status.failed > 0 {
                self.status_message = Some(format!(
                    "Tasks: {} done, {} failed",
                    status.completed, status.failed
                ));
            }
        }
    }

    /// Get or create the Witch.
    pub(super) fn witch(&mut self) -> &mut crate::witch::Witch {
        if self.witch.is_none() {
            let force_freshen = self.config.opinions.startup.freshen_last_stage_at_startup;
            let force_check = self.config.opinions.startup.force_check_all_files_at_startup;
            let log_rx = self.log_rx.take();
            self.witch = Some(crate::witch::Witch::with_opinions(&self.config, false, force_freshen, force_check, log_rx));
        }
        self.witch.as_mut().unwrap()
    }

    /// Shorthand for read-only database access.
    ///
    /// Returns a `ReadOnlyDb` wrapper with common query methods:
    /// ```ignore
    /// let read_db = app.read_db();
    /// let audio_files = read_db.get_all_audio_files(FileSource::Corpus)?;
    /// let signals = read_db.get_signals(None)?;
    /// ```
    pub(super) fn read_db(&mut self) -> crate::corpus::db::ReadOnlyDb<'_> {
        self.witch().read_db()
    }

}

// ============================================================================
// Rendering
// ============================================================================

fn render(f: &mut Frame, app: &mut App) {
    // Use cached corpus summary from the Witch's background cache.
    // Never queries DB - just reads whatever was last computed.
    let corpus_summary = app
        .witch
        .as_ref()
        .and_then(|d| d.ui_read_cache().corpus_summary());

    // Fetch DB thread stats (cheap - just reads cached atomic values)
    // Returns None if timing instrumentation is disabled
    let db_stats = app.witch.as_ref().and_then(|d| d.db_stats());

    // Get Witch status WITHOUT ticking again - status() just reads current state
    let witch_status = app.witch.as_ref().and_then(|w| {
        let status = w.status();
        // Show if there's pending work OR a lingering completed session
        if status.pending > 0 || status.completed_session.is_some() {
            Some(status)
        } else {
            None
        }
    });

    // Fetch decision summaries from Witch if transaction review is active
    let transaction_review_decisions = if app.transaction_review.is_some() {
        app.witch.as_ref()
            .map(|w| transaction_review::fetch_decision_summaries(w))
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    // Build status bar lines. Status message overrides line 1; otherwise
    // the active mode provides contextual content (e.g. selected file path).
    let status_line_1 = if let Some(ref msg) = app.status_message {
        Some(msg.clone())
    } else {
        match app.mode {
            UiMode::CorpusBrowser => app.tree_browser.as_ref()
                .and_then(|b| b.selected_path())
                .map(|p| p.to_string_lossy().to_string()),
            UiMode::TagCanonicityResolution => app.tag_canonicity_state.as_ref()
                .and_then(|s| s.data.files.get(s.file_cursor))
                .map(|f| f.path.clone()),
            _ => None,
        }
    };

    let status_line_2 = app.witch.as_ref()
        .and_then(|w| w.transaction_summary())
        .map(|(label, dec, mut_)| {
            let pd = if dec == 1 { "" } else { "s" };
            let pm = if mut_ == 1 { "" } else { "s" };
            format!("Transaction \"{label}\": {dec} decision{pd}, {mut_} mutation{pm} staged")
        });

    let mut ctx = render::RenderContext {
        mode: app.mode,
        tree_browser: app.tree_browser.as_mut(),
        deployment_preview: app.deployment_preview.as_mut(),
        missing_file_preview: app.missing_file_preview.as_ref(),
        missing_directory_preview: app.missing_directory_preview.as_ref(),
        tag_canonicity_state: app.tag_canonicity_state.as_ref(),
        compound_split_state: app.compound_split_state.as_ref(),
        oob_sync_state: app.oob_sync_state.as_mut(),
        oob_conflict_state: app.oob_conflict_state.as_mut(),
        moved_file_state: app.moved_file_state.as_mut(),
        transaction_review: app.transaction_review.as_ref(),
        transaction_review_decisions,
        exit_confirm_modal_state: app.exit_confirm_modal_state.as_ref(),
        progress_screen: app.progress_screen.as_ref(),
        insights_view: app.insights_view.as_mut(),
        tag_search: app.tag_search.as_ref(),
        intake_confirmation: app.intake_confirmation.as_ref(),
        corrupt_file_preview: app.corrupt_file_preview.as_ref(),
        shit_format_preview: app.shit_format_preview.as_ref(),
        subpar_duplicate_preview: app.subpar_duplicate_preview.as_ref(),
        directory_cluster_preview: app.directory_cluster_preview.as_ref(),
        progressive_worker: app.progressive_worker.as_ref(),
        unified_tag_editor: app.unified_tag_editor.as_mut(),
        eye: &app.eye,
        witch_status,
        corpus_summary,
        db_stats,
        filter_popup_state: app.filter_popup_state.as_ref(),
        status_line_1,
        status_line_2,
    };
    render::render(f, &mut ctx);
}

// ============================================================================
// Entry Point
// ============================================================================

pub fn run_menu(config: Config, log_rx: std::sync::mpsc::Receiver<crate::logging::LogOp>) -> Result<()> {
    // Log startup
    crate::logging::log_general("=== MLA startup ===");

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Determine initial state based on database
    let db_path = crate::config::get_db_path()?;
    let initial_state = crate::witch::InitialUiState::determine(&db_path);

    // Handle first-time setup (no Witch needed - creates the database)
    if initial_state == crate::witch::InitialUiState::FirstTimeSetup {
        startup::handle_first_time_setup(&mut terminal, &db_path)?;
    }

    // Create Witch early (without db_thread yet)
    // The Witch handles migrations via rayon tasks, then spawns db_thread after.
    let force_freshen = config.opinions.startup.freshen_last_stage_at_startup;
    let force_check = config.opinions.startup.force_check_all_files_at_startup;
    let mut witch = crate::witch::Witch::with_opinions(&config, false, force_freshen, force_check, Some(log_rx));

    // Run migrations if needed (self-contained loop with its own UI)
    if witch.needs_migrations() {
        startup::run_migration_flow(&mut terminal, &mut witch)?;
    }

    // NOW spawn db_thread - schema is guaranteed correct
    witch.spawn_db_thread();

    // Create App with pre-existing Witch
    let mut app = App::new_with_witch(config, witch);

    // Observing ALWAYS runs at startup
    // Start observing via the Witch - this sets observation_state and queues work
    app.witch().start_observing();

    app.progress_screen = Some(progress_screen::ProgressScreen::new_eyeballing());
    app.mode = UiMode::Progress;

    let res = run_app(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
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
    // Set up signal handling - treat SIGINT/SIGTERM as ESC press
    let signal_received = Arc::new(AtomicBool::new(false));

    // Register signal handlers
    if let Err(e) = register_signal_handlers(Arc::clone(&signal_received)) {
        crate::logging::log_error(format!(
            "Failed to register signal handlers: {}",
            e
        ));
    }

    loop {
        let frame_start = std::time::Instant::now();

        // Check if a signal was received - treat as ESC
        if signal_received.swap(false, Ordering::SeqCst) {
            let esc_key = KeyEvent::new(KeyCode::Esc, crossterm::event::KeyModifiers::NONE);
            app.handle_key(esc_key);
        }

        // Tick the Witch first - She is the driving system
        let tick_start = std::time::Instant::now();
        let tick_status = app.witch().tick();
        let tick_duration = tick_start.elapsed();
        if tick_duration.as_millis() > 16 {
            crate::logging::log_perf(format!(
                "[FRAME DEBUG] witch.tick() took {}ms, drained {} results",
                tick_duration.as_millis(),
                tick_status.total_processed
            ));
        }

        app.check_witch_status();

        // Update eye animation - only animate when the Witch's eye is Awake
        let can_animate = app.witch().eye_state() == crate::witch::EyeState::Awake;
        app.eye.update(can_animate);

        // Update insights view with the Witch's status and cached data
        if let Some(ref mut view) = app.insights_view {
            let status = app.witch.as_ref().map(|d| d.status());
            let insights_data = app.witch.as_ref().and_then(|w| w.ui_read_cache().insights_data());
            view.update(status.as_ref(), insights_data);
        }

        // Tick progress screen if active (startup eyeballing, content analysis, etc.)
        if app.progress_screen.is_some() {
            app.tick_progress_screen();
        }

        // Tick progressive worker if active (bulk operations with progress bar)
        if app.progressive_worker.is_some() {
            app.tick_progressive_worker();
        }

        // Flag demand for cached UI data (the Witch spawns background refresh if needed)
        if let Some(ref the_witch) = app.witch {
            the_witch.ui_read_cache().want_corpus_summary();
            // Request insights data when viewing insights
            if app.mode == UiMode::Insights {
                the_witch.ui_read_cache().want_insights_data();
            }
        }

        let draw_start = std::time::Instant::now();
        terminal.draw(|f| render(f, app))?;
        let draw_duration = draw_start.elapsed();
        if draw_duration.as_millis() > 16 {
            crate::logging::log_perf(format!(
                "[FRAME DEBUG] terminal.draw() took {}ms",
                draw_duration.as_millis()
            ));
        }

        let frame_duration = frame_start.elapsed();
        if frame_duration.as_millis() > 16 {
            crate::logging::log_perf(format!(
                "[FRAME DEBUG] SLOW FRAME: total {}ms (tick={}ms, draw={}ms)",
                frame_duration.as_millis(),
                tick_duration.as_millis(),
                draw_duration.as_millis()
            ));
        }

        // Tick tag search for pending bulk edit (after modal has rendered)
        app.tick_tag_search();

        if event::poll(std::time::Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) => {
                    // Ctrl+C: treat as ESC for graceful exit handling
                    if key.code == KeyCode::Char('c')
                        && key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL)
                    {
                        let esc_key = KeyEvent::new(KeyCode::Esc, crossterm::event::KeyModifiers::NONE);
                        app.handle_key(esc_key);
                    } else {
                        app.handle_key(key);
                    }
                }
                Event::Mouse(mouse) => {
                    match mouse.kind {
                        // Map scroll wheel to arrow keys
                        MouseEventKind::ScrollUp => {
                            let key = KeyEvent::new(KeyCode::Up, crossterm::event::KeyModifiers::NONE);
                            app.handle_key(key);
                        }
                        MouseEventKind::ScrollDown => {
                            let key = KeyEvent::new(KeyCode::Down, crossterm::event::KeyModifiers::NONE);
                            app.handle_key(key);
                        }
                        // Handle left mouse button clicks for button detection
                        MouseEventKind::Down(MouseButton::Left) => {
                            app.handle_click(mouse.column, mouse.row);
                        }
                        _ => {} // Ignore other mouse events
                    }
                }
                _ => {} // Ignore other events (resize, etc.)
            }
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}

/// Register signal handlers for graceful shutdown.
///
/// SIGINT (Ctrl+C) and SIGTERM are caught and converted to ESC key presses
/// so the app can handle them gracefully (show exit confirmation, etc.).
fn register_signal_handlers(flag: Arc<AtomicBool>) -> Result<(), Box<dyn std::error::Error>> {
    use signal_hook::consts::{SIGINT, SIGTERM};
    use signal_hook::flag;

    // Register both SIGINT and SIGTERM to set the flag
    flag::register(SIGINT, Arc::clone(&flag))?;
    flag::register(SIGTERM, flag)?;

    Ok(())
}
