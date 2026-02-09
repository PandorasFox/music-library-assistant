//! Main rendering functions for the TUI.
//!
//! This module contains all top-level render functions that dispatch
//! to mode-specific renderers.

use std::time::Instant;

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use super::app::{EyeAnimation, EyeFrame, EYE_CLOSED, EYE_CLOSING, EYE_OPEN};
use super::widgets::{status_bar, Modal, ModalButton, ModalStyle};
use super::{compound_split_v2, corrupt_file_flow, deploy_flow, directory_cluster_flow, filter_popup, insights_view, missing_file_flow, oob_conflict_flow, oob_sync_flow, progressive_worker, shit_format_flow, subpar_duplicate_flow, tag_canonicity_v2, tag_editor, tag_search, transaction_review, tree_browser};

/// Display context passed to rendering functions.
/// Contains all the state needed to render the UI.
pub struct RenderContext<'a> {
    pub mode: super::UiMode,
    pub tree_browser: Option<&'a mut tree_browser::TreeBrowserState>,
    pub deployment_preview: Option<&'a mut deploy_flow::DeploymentPreviewState>,
    pub missing_file_preview: Option<&'a missing_file_flow::MissingFilePreviewState>,
    pub missing_directory_preview: Option<&'a super::missing_directory_flow::MissingDirectoryPreviewState>,
    pub tag_canonicity_state: Option<&'a tag_canonicity_v2::TagCanonicalityStateV2>,
    pub compound_split_state: Option<&'a compound_split_v2::CompoundSplitStateV2>,
    pub oob_sync_state: Option<&'a mut oob_sync_flow::OobSyncState>,
    pub oob_conflict_state: Option<&'a mut oob_conflict_flow::OobConflictState>,
    pub moved_file_state: Option<&'a mut super::moved_file_flow::MovedFileState>,
    pub transaction_review: Option<&'a transaction_review::TransactionReviewState>,
    pub transaction_review_decisions: Vec<transaction_review::DecisionSummary>,
    pub unified_tag_editor: Option<&'a mut tag_editor::UnifiedTagEditorState>,
    pub exit_confirm_modal_state: Option<&'a super::ExitConfirmModalState>,
    pub progress_screen: Option<&'a super::progress_screen::ProgressScreen>,
    pub insights_view: Option<&'a mut insights_view::InsightsViewState>,
    pub tag_search: Option<&'a tag_search::TagSearchState>,
    pub intake_confirmation: Option<&'a super::startup::IntakeConfirmationState>,
    pub corrupt_file_preview: Option<&'a corrupt_file_flow::CorruptFilePreviewState>,
    pub shit_format_preview: Option<&'a shit_format_flow::ShitFormatPreviewState>,
    pub subpar_duplicate_preview: Option<&'a subpar_duplicate_flow::SubparDuplicatePreviewState>,
    pub directory_cluster_preview: Option<&'a directory_cluster_flow::DirectoryClusterPreviewState>,
    pub progressive_worker: Option<&'a progressive_worker::ProgressiveWorkerState>,
    pub eye: &'a EyeAnimation,
    pub witch_status: Option<crate::witch::DaemonStatus>,
    pub corpus_summary: Option<crate::meta::signals::CorpusSummary>,
    pub db_stats: Option<crate::db_thread::DbThreadStats>,
    pub filter_popup_state: Option<&'a filter_popup::FilterPopupState>,
    /// Status bar line 1 content (path, info, etc.)
    pub status_line_1: Option<String>,
    /// Status bar line 2 content (transaction info, etc.)
    pub status_line_2: Option<String>,
}

/// Main render entry point - dispatches to sub-renderers based on mode.
pub fn render(f: &mut Frame, ctx: &mut RenderContext) {
    // Progress screen takes the whole screen (startup, content analysis, signal refresh)
    if ctx.mode == super::UiMode::Progress {
        if let Some(progress) = ctx.progress_screen {
            // For non-eyeballing phases, provide animated eye frame
            let eye_frame = if progress.uses_closed_eye() {
                None // Uses its own closed/awakening eye
            } else {
                // Awake phase - use animated eye
                Some(match ctx.eye.current_frame() {
                    EyeFrame::Open => EYE_OPEN,
                    EyeFrame::Closing => EYE_CLOSING,
                    EyeFrame::Closed => EYE_CLOSED,
                })
            };
            let start = Instant::now();
            super::progress_screen::render(f, f.area(), progress, eye_frame);
            let elapsed = start.elapsed();
            if elapsed.as_millis() > 16 {
                crate::logging::log_perf(format!(
                    "[RENDER DEBUG] progress_screen::render took {}ms",
                    elapsed.as_millis()
                ));
            }
        }
        return;
    }

    // Lateral views (Tag Search, Corpus Browser, Insights, Deploy) have their own title bar
    // and get the full header+content area
    // DeploymentPreview is NOT part of the lateral ring anymore - it renders its own title
    let uses_unified_titlebar = matches!(
        ctx.mode,
        super::UiMode::TagSearch
            | super::UiMode::CorpusBrowser
            | super::UiMode::Insights
    );

    if uses_unified_titlebar {
        // Two-part layout: content (with unified titlebar) + status bar
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(10),   // Content with unified titlebar
                Constraint::Length(2), // Status bar
            ])
            .split(f.area());

        let start = Instant::now();
        render_content(f, chunks[0], ctx);
        let content_time = start.elapsed();

        let start = Instant::now();
        render_status_bar(f, chunks[1], ctx);
        let footer_time = start.elapsed();

        if content_time.as_millis() > 16 || footer_time.as_millis() > 16 {
            crate::logging::log_perf(format!(
                "[RENDER DEBUG] mode={:?} content={}ms footer={}ms",
                ctx.mode,
                content_time.as_millis(),
                footer_time.as_millis()
            ));
        }
    } else {
        // Standard three-part layout: header + content + status bar
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // Header
                Constraint::Min(10),   // Content
                Constraint::Length(2), // Status bar
            ])
            .split(f.area());

        let start = Instant::now();
        render_header(f, chunks[0], ctx);
        let header_time = start.elapsed();

        let start = Instant::now();
        render_content(f, chunks[1], ctx);
        let content_time = start.elapsed();

        let start = Instant::now();
        render_status_bar(f, chunks[2], ctx);
        let footer_time = start.elapsed();

        if header_time.as_millis() > 16 || content_time.as_millis() > 16 || footer_time.as_millis() > 16 {
            crate::logging::log_perf(format!(
                "[RENDER DEBUG] mode={:?} header={}ms content={}ms footer={}ms",
                ctx.mode,
                header_time.as_millis(),
                content_time.as_millis(),
                footer_time.as_millis()
            ));
        }
    }
}

fn render_header(f: &mut Frame, area: ratatui::layout::Rect, ctx: &RenderContext) {
    // Get mode-specific suffix (if any)
    let suffix = match ctx.mode {
        super::UiMode::Progress => None, // Never reached - handled separately
        super::UiMode::ProgressiveWork => Some("Processing"),
        super::UiMode::DeploymentPreview => Some("Deployment Preview"),
        super::UiMode::ExitConfirmModal => Some("Exit Confirmation"),
        super::UiMode::CorpusBrowser => Some("Corpus Browser"),
        super::UiMode::Insights => Some("Corpus Insights"),
        super::UiMode::IntakeConfirmation => Some("Intake Confirmation"),
        super::UiMode::TagSearch => Some("Tag Search"),
        super::UiMode::UnifiedTagEditor => Some("Tag Editor"),
        super::UiMode::MissingFileResolution => Some("Missing File Resolution"),
        super::UiMode::MissingDirectoryResolution => Some("Missing Directory Acknowledgment"),
        super::UiMode::TagCanonicityResolution => Some("Tag Canonicity"),
        super::UiMode::CompoundTagSplit => Some("Compound Tag Split"),
        super::UiMode::OobSyncResolution => Some("OOB Tag Sync"),
        super::UiMode::OobConflictInspection => Some("OOB Tag Conflicts"),
        super::UiMode::MovedFileAcknowledge => Some("Moved Files"),
        super::UiMode::TransactionReview => Some("Transaction Review"),
        super::UiMode::CorruptFileResolution => Some("Corrupt File Resolution"),
        super::UiMode::ShitFormatResolution => Some("Shit Format Resolution"),
        super::UiMode::SubparDuplicateResolution => Some("Subpar Duplicate Resolution"),
        super::UiMode::DirectoryClusterResolution => Some("Directory Overlap Resolution"),
    };

    let title = match suffix {
        Some(s) => format!("{} - {}", crate::MLA_TITLE, s),
        None => crate::MLA_TITLE.to_string(),
    };

    let header = Paragraph::new(title)
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));

    f.render_widget(header, area);
}

fn render_content(f: &mut Frame, area: ratatui::layout::Rect, ctx: &mut RenderContext) {
    let start = Instant::now();
    let view_name: &str;

    match ctx.mode {
        super::UiMode::Progress => {
            // Never reached - handled separately in render() before this function
            view_name = "progress";
        }
        super::UiMode::ProgressiveWork => {
            view_name = "progressive_work";
            if let Some(ref state) = ctx.progressive_worker {
                progressive_worker::render(f, area, state);
            }
        }
        super::UiMode::DeploymentPreview => {
            view_name = "deployment_preview";
            if let Some(ref mut preview) = ctx.deployment_preview {
                preview.render(f, area);
            }
        }
        super::UiMode::ExitConfirmModal => {
            view_name = "exit_confirm_modal";
            render_exit_confirm_modal(f, area, ctx.exit_confirm_modal_state);
        }
        super::UiMode::CorpusBrowser => {
            view_name = "corpus_browser";
            if let Some(ref mut browser) = ctx.tree_browser {
                browser.render(f, area);
            }
        }
        super::UiMode::Insights => {
            view_name = "insights";
            if let Some(ref mut view) = ctx.insights_view {
                insights_view::render_insights_view(f, area, view);
            } else {
                // Show loading state while insights view is initializing
                render_insights_loading(f, area);
            }
        }
        super::UiMode::TagSearch => {
            view_name = "tag_search";
            if let Some(ref state) = ctx.tag_search {
                state.render(f, area);
            }
        }
        super::UiMode::IntakeConfirmation => {
            view_name = "intake_confirmation";
            if let Some(ref state) = ctx.intake_confirmation {
                super::startup::intake_confirmation::render(f, area, state);
            }
        }
        super::UiMode::UnifiedTagEditor => {
            view_name = "unified_tag_editor";
            if let Some(ref mut editor) = ctx.unified_tag_editor {
                editor.render(f, area);
            }
        }
        super::UiMode::MissingFileResolution => {
            view_name = "missing_file_resolution";
            if let Some(ref preview) = ctx.missing_file_preview {
                preview.render(f, area);
            }
        }
        super::UiMode::MissingDirectoryResolution => {
            view_name = "missing_directory_resolution";
            if let Some(ref preview) = ctx.missing_directory_preview {
                preview.render(f, area);
            }
        }
        super::UiMode::TagCanonicityResolution => {
            view_name = "tag_canonicity_resolution";
            if let Some(ref state) = ctx.tag_canonicity_state {
                tag_canonicity_v2::render(f, area, state);
            }
        }
        super::UiMode::CompoundTagSplit => {
            view_name = "compound_tag_split";
            if let Some(ref state) = ctx.compound_split_state {
                compound_split_v2::render(f, area, state);
            }
        }
        super::UiMode::OobSyncResolution => {
            view_name = "oob_sync_resolution";
            if let Some(ref mut state) = ctx.oob_sync_state {
                oob_sync_flow::render(f, area, state);
            }
        }
        super::UiMode::OobConflictInspection => {
            view_name = "oob_conflict_inspection";
            if let Some(ref mut state) = ctx.oob_conflict_state {
                oob_conflict_flow::render(f, area, state);
            }
        }
        super::UiMode::MovedFileAcknowledge => {
            view_name = "moved_file_acknowledge";
            if let Some(ref mut state) = ctx.moved_file_state {
                super::moved_file_flow::render(state, f, area);
            }
        }
        super::UiMode::TransactionReview => {
            view_name = "transaction_review";
            if let Some(ref state) = ctx.transaction_review {
                transaction_review::render(f, area, state, &ctx.transaction_review_decisions);
            }
        }
        super::UiMode::CorruptFileResolution => {
            view_name = "corrupt_file_resolution";
            if let Some(ref preview) = ctx.corrupt_file_preview {
                preview.render(f, area);
            }
        }
        super::UiMode::ShitFormatResolution => {
            view_name = "shit_format_resolution";
            if let Some(ref preview) = ctx.shit_format_preview {
                preview.render(f, area);
            }
        }
        super::UiMode::SubparDuplicateResolution => {
            view_name = "subpar_duplicate_resolution";
            if let Some(ref preview) = ctx.subpar_duplicate_preview {
                preview.render(f, area);
            }
        }
        super::UiMode::DirectoryClusterResolution => {
            view_name = "directory_cluster_resolution";
            if let Some(ref preview) = ctx.directory_cluster_preview {
                preview.render(f, area);
            }
        }
    }

    // Render filter popup overlay if active
    if let Some(ref popup_state) = ctx.filter_popup_state {
        filter_popup::render(f, area, popup_state);
    }

    let elapsed = start.elapsed();
    if elapsed.as_millis() > 16 {
        crate::logging::log_perf(format!(
            "[RENDER DEBUG] render_content({}) took {}ms",
            view_name,
            elapsed.as_millis()
        ));
    }
}

/// Render loading state for Insights view while initializing
fn render_insights_loading(f: &mut Frame, area: ratatui::layout::Rect) {
    use super::widgets::{LateralView, UnifiedTitleBar};

    // Layout: unified titlebar + content
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(UnifiedTitleBar::height()),
            Constraint::Min(0),
        ])
        .split(area);

    // Render unified titlebar
    let titlebar = UnifiedTitleBar::new(LateralView::Insights);
    titlebar.render(f, chunks[0]);

    // Render loading message
    let loading = Paragraph::new("Initializing insights view...")
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL).title("Insights"));

    f.render_widget(loading, chunks[1]);
}

fn render_exit_confirm_modal(
    f: &mut Frame,
    area: ratatui::layout::Rect,
    state: Option<&super::ExitConfirmModalState>,
) {
    let selected_no = state.map(|s| s.selected_no).unwrap_or(true);
    let has_operations = state.map(|s| s.has_operations).unwrap_or(false);

    // Build buttons with appropriate styles
    let (yes_btn, no_btn) = if has_operations {
        // Warning modal: Yes is red (dangerous), No is green (safe)
        let yes_btn = ModalButton::new("Yes", "")
            .with_indicator()
            .styles(
                Style::default().fg(Color::Black).bg(Color::Red),
                Style::default().fg(Color::White),
            );
        let no_btn = ModalButton::new("No", "")
            .with_indicator()
            .styles(
                Style::default().fg(Color::Black).bg(Color::Green),
                Style::default().fg(Color::White),
            );
        if selected_no {
            (yes_btn, no_btn.selected())
        } else {
            (yes_btn.selected(), no_btn)
        }
    } else {
        // Simple exit: neutral styles
        let confirm_btn = ModalButton::new("Confirm", "")
            .with_indicator()
            .styles(
                Style::default().fg(Color::Black).bg(Color::Cyan),
                Style::default().fg(Color::White),
            );
        let cancel_btn = ModalButton::new("Cancel", "")
            .with_indicator()
            .styles(
                Style::default().fg(Color::Black).bg(Color::Gray),
                Style::default().fg(Color::White),
            );
        if selected_no {
            (confirm_btn, cancel_btn.selected())
        } else {
            (confirm_btn.selected(), cancel_btn)
        }
    };

    // Build button line
    let mut button_spans = yes_btn.render_with_indicator();
    button_spans.push(Span::raw("     "));
    button_spans.extend(no_btn.render_with_indicator());
    let button_line = Line::from(button_spans);

    if has_operations {
        // Warning modal for operations in progress
        let content = vec![
            Line::from(""),
            Line::from(Span::styled(
                "Operation In Progress",
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from("An operation is currently running."),
            Line::from("Exiting now may leave your library"),
            Line::from("in an inconsistent state."),
            Line::from(""),
            Line::from(Span::styled(
                "No guarantees about safety or integrity.",
                Style::default().fg(Color::Red),
            )),
            Line::from(""),
            Line::from(""),
            Line::from("Exit anyway?"),
            Line::from(""),
            button_line,
        ];

        Modal::new()
            .title(" Warning ")
            .content(content)
            .fixed_size(50, 16)
            .style(ModalStyle::warning())
            .centered()
            .render(f, area);
    } else {
        // Simple exit confirmation
        let content = vec![
            Line::from(""),
            Line::from("Exit MLA?"),
            Line::from(""),
            Line::from(""),
            button_line,
        ];

        Modal::new()
            .title(" Exit ")
            .content(content)
            .fixed_size(40, 9)
            .style(ModalStyle::info())
            .centered()
            .render(f, area);
    }
}

/// Render the minimal 2-line status bar.
fn render_status_bar(f: &mut Frame, area: ratatui::layout::Rect, ctx: &RenderContext) {
    status_bar::render(
        f,
        area,
        ctx.status_line_1.as_deref(),
        ctx.status_line_2.as_deref(),
    );
}

