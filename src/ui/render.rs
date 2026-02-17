//! Main rendering functions for the TUI.
//!
//! This module contains all top-level render functions that dispatch
//! to view-specific renderers based on the ActiveView enum.

use std::time::Instant;

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use super::active_view::{ActiveView, ExitConfirmModalState};
use super::eye::{EyeFrame, EYE_CLOSED, EYE_CLOSING, EYE_OPEN};
use super::startup;
use super::widgets::{status_bar, Modal, ModalButton, ModalStyle, UnifiedTitleBar};
use super::{
    compound_split_v2, config_editor, filter_popup, inbox_view, insights_view, manual_review_modal,
    oob_conflict_modal, oob_sync_modal, progressive_worker, tag_canonicity_v2,
    transaction_review, transaction_view,
};

/// Main render entry point - dispatches to sub-renderers based on ActiveView.
pub fn render_app(
    f: &mut Frame,
    app: &mut super::App,
    transaction_review_decisions: Vec<transaction_review::DecisionSummary>,
    status_line_1: Option<String>,
    status_line_2: Option<String>,
) {
    // Progress screen takes the whole screen (startup, content analysis, signal refresh)
    if let ActiveView::Progress { ref screen, ref eye } = app.view {
        // For non-eyeballing phases, provide animated eye frame
        let eye_frame = if screen.uses_closed_eye() {
            None // Uses its own closed/awakening eye
        } else {
            // Awake phase - use animated eye
            Some(match eye.current_frame() {
                EyeFrame::Open => EYE_OPEN,
                EyeFrame::Closing => EYE_CLOSING,
                EyeFrame::Closed => EYE_CLOSED,
            })
        };
        let start = Instant::now();
        super::progress_screen::render(f, f.area(), screen, eye_frame);
        let elapsed = start.elapsed();
        if elapsed.as_millis() > 16 {
            crate::logging::log_perf(format!(
                "[RENDER DEBUG] progress_screen::render took {}ms",
                elapsed.as_millis()
            ));
        }
        return;
    }

    // Lateral views have a unified titlebar rendered centrally here
    let lateral_view = app.view.lateral_view();

    if let Some(lv) = lateral_view {
        // Three-part layout: titlebar + content + status bar
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(UnifiedTitleBar::height()), // Titlebar
                Constraint::Min(10),                           // Content
                Constraint::Length(2),                         // Status bar
            ])
            .split(f.area());

        // Read deploy_needs_action from UiCache for titlebar
        let deploy_needs_action = app.witch.ui_read_cache().deploy_status()
            .map_or(false, |s| s.needs_action);

        let transactions_open = app.config().opinions.leave_transactions_open;
        let transaction_has_decisions = app.witch.has_transaction() && app.witch.transaction_summary().is_some_and(|(_, d, _)| d > 0);

        let titlebar = UnifiedTitleBar::new(lv)
            .with_deploy_needs_action(deploy_needs_action)
            .with_transactions_open(transactions_open)
            .with_transaction_has_decisions(transaction_has_decisions);
        titlebar.render(f, chunks[0]);

        let start = Instant::now();
        render_content(f, app, &transaction_review_decisions, chunks[1]);
        let content_time = start.elapsed();

        let start = Instant::now();
        render_status_bar(f, chunks[2], status_line_1.as_deref(), status_line_2.as_deref());
        let footer_time = start.elapsed();

        if content_time.as_millis() > 16 || footer_time.as_millis() > 16 {
            crate::logging::log_perf(format!(
                "[RENDER DEBUG] view={} content={}ms footer={}ms",
                view_name(&app.view),
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
        render_header(f, chunks[0], &app.view);
        let header_time = start.elapsed();

        let start = Instant::now();
        render_content(f, app, &transaction_review_decisions, chunks[1]);
        let content_time = start.elapsed();

        let start = Instant::now();
        render_status_bar(f, chunks[2], status_line_1.as_deref(), status_line_2.as_deref());
        let footer_time = start.elapsed();

        if header_time.as_millis() > 16 || content_time.as_millis() > 16 || footer_time.as_millis() > 16 {
            crate::logging::log_perf(format!(
                "[RENDER DEBUG] view={} header={}ms content={}ms footer={}ms",
                view_name(&app.view),
                header_time.as_millis(),
                content_time.as_millis(),
                footer_time.as_millis()
            ));
        }
    }
}

fn render_header(f: &mut Frame, area: ratatui::layout::Rect, view: &ActiveView) {
    let suffix = view.header_suffix();

    let title = match suffix {
        Some(s) => format!("{} - {}", crate::MM_TITLE, s),
        None => crate::MM_TITLE.to_string(),
    };

    let header = Paragraph::new(title)
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));

    f.render_widget(header, area);
}

fn render_content(
    f: &mut Frame,
    app: &mut super::App,
    transaction_review_decisions: &[transaction_review::DecisionSummary],
    area: ratatui::layout::Rect,
) {
    let start = Instant::now();
    let vname: &str;

    match app.view {
        ActiveView::MigrationApproval(ref state) => {
            vname = "migration_approval";
            startup::migrations::render_migration_view(f, area, state);
        }
        ActiveView::VacuumPrompt(ref state) => {
            vname = "vacuum_prompt";
            startup::vacuum::render_vacuum_view(f, area, state);
        }
        ActiveView::Progress { .. } => {
            // Never reached - handled separately in render_app() before this function
            vname = "progress";
        }
        ActiveView::ConfigEditor(ref state) => {
            vname = "config_editor";
            config_editor::render::render(f, area, state);
        }
        ActiveView::ProgressiveWork(ref worker) => {
            vname = "progressive_work";
            progressive_worker::render(f, area, worker);
        }
        ActiveView::Deploy(ref state) => {
            vname = "deploy";
            state.render(f, area);
        }
        ActiveView::ExitConfirm(ref state) => {
            vname = "exit_confirm_modal";
            render_exit_confirm_modal(f, area, state);
        }
        ActiveView::CorpusBrowser(ref mut browser) => {
            vname = "corpus_browser";
            browser.render(f, area);
        }
        ActiveView::Insights(ref mut view) => {
            vname = "insights";
            insights_view::render_insights_view(f, area, view);
        }
        ActiveView::Inbox(ref state) => {
            vname = "inbox";
            inbox_view::render_inbox_view(f, area, state);
        }
        ActiveView::Transaction(ref state) => {
            vname = "transaction";
            let decisions = transaction_review::fetch_decision_summaries(&app.witch);
            transaction_view::render::render(f, area, state, &decisions);
        }
        ActiveView::TagSearch(ref state) => {
            vname = "tag_search";
            state.render(f, area);
        }
        ActiveView::IntakeConfirmation(ref state) => {
            vname = "intake_confirmation";
            super::startup::intake_confirmation::render(f, area, state);
        }
        ActiveView::UnifiedTagEditor(ref mut editor) => {
            vname = "unified_tag_editor";
            editor.render(f, area);
        }
        ActiveView::MissingFileResolution(ref preview) => {
            vname = "missing_file_resolution";
            preview.render(f, area);
        }
        ActiveView::MissingDirectoryResolution(ref preview) => {
            vname = "missing_directory_resolution";
            preview.render(f, area);
        }
        ActiveView::TagCanonicityResolution { ref state, .. } => {
            vname = "tag_canonicity_resolution";
            tag_canonicity_v2::render(f, area, state);
        }
        ActiveView::CompoundTagSplit { ref state, .. } => {
            vname = "compound_tag_split";
            compound_split_v2::render(f, area, state);
        }
        ActiveView::OobSyncResolution(ref mut state) => {
            vname = "oob_sync_resolution";
            oob_sync_modal::render(f, area, state);
        }
        ActiveView::OobConflictInspection(ref mut state) => {
            vname = "oob_conflict_inspection";
            oob_conflict_modal::render(f, area, state);
        }
        ActiveView::MovedFileAcknowledge(ref mut state) => {
            vname = "moved_file_acknowledge";
            super::moved_file_modal::render(state, f, area);
        }
        ActiveView::TransactionReview(ref review) => {
            vname = "transaction_review";
            transaction_review::render(f, area, review, transaction_review_decisions);
        }
        ActiveView::CorruptFileResolution(ref preview) => {
            vname = "corrupt_file_resolution";
            preview.render(f, area);
        }
        ActiveView::ShitFormatResolution(ref preview) => {
            vname = "shit_format_resolution";
            preview.render(f, area);
        }
        ActiveView::EmbedAlbumArtResolution(ref preview) => {
            vname = "embed_album_art_resolution";
            preview.render(f, area);
        }
        ActiveView::SubparDuplicateResolution(ref preview) => {
            vname = "subpar_duplicate_resolution";
            preview.render(f, area);
        }
        ActiveView::InboxCorpusMatchResolution(ref preview) => {
            vname = "inbox_corpus_match_resolution";
            preview.render(f, area);
        }
        ActiveView::InboxOrganize(ref mut state) => {
            vname = "inbox_organize";
            super::inbox_organize::render::render(f, area, state);
        }
        ActiveView::DirectoryClusterResolution(ref preview) => {
            vname = "directory_cluster_resolution";
            preview.render(f, area);
        }
        ActiveView::MissingAlbumSingleResolution(ref state) => {
            vname = "missing_album_single";
            state.render(f, area);
        }
        ActiveView::ManualReview(ref state) => {
            vname = "manual_review";
            manual_review_modal::render(f, area, state);
        }
    }

    // Render filter popup overlay if active
    if let Some(ref overlay) = app.filter_overlay {
        filter_popup::render(f, area, &overlay.state);
    }

    let elapsed = start.elapsed();
    if elapsed.as_millis() > 16 {
        crate::logging::log_perf(format!(
            "[RENDER DEBUG] render_content({}) took {}ms",
            vname,
            elapsed.as_millis()
        ));
    }
}

fn render_exit_confirm_modal(
    f: &mut Frame,
    area: ratatui::layout::Rect,
    state: &ExitConfirmModalState,
) {
    let selected_no = state.selected_no;
    let has_operations = state.has_operations;

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
            Line::from("Exit MM?"),
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
fn render_status_bar(
    f: &mut Frame,
    area: ratatui::layout::Rect,
    status_line_1: Option<&str>,
    status_line_2: Option<&str>,
) {
    status_bar::render(f, area, status_line_1, status_line_2);
}

/// Get a short name for the active view (for perf logging).
fn view_name(view: &ActiveView) -> &'static str {
    match view {
        ActiveView::MigrationApproval(_) => "migration_approval",
        ActiveView::VacuumPrompt(_) => "vacuum_prompt",
        ActiveView::ConfigEditor(_) => "config_editor",
        ActiveView::Insights(_) => "insights",
        ActiveView::Inbox(_) => "inbox",
        ActiveView::Transaction(_) => "transaction",
        ActiveView::CorpusBrowser(_) => "corpus_browser",
        ActiveView::TagSearch(_) => "tag_search",
        ActiveView::Progress { .. } => "progress",
        ActiveView::ProgressiveWork(_) => "progressive_work",
        ActiveView::ExitConfirm(_) => "exit_confirm",
        ActiveView::IntakeConfirmation(_) => "intake_confirmation",
        ActiveView::UnifiedTagEditor(_) => "unified_tag_editor",
        ActiveView::Deploy(_) => "deploy",
        ActiveView::MissingFileResolution(_) => "missing_file_resolution",
        ActiveView::MissingDirectoryResolution(_) => "missing_directory_resolution",
        ActiveView::CorruptFileResolution(_) => "corrupt_file_resolution",
        ActiveView::ShitFormatResolution(_) => "shit_format_resolution",
        ActiveView::EmbedAlbumArtResolution(_) => "embed_album_art_resolution",
        ActiveView::SubparDuplicateResolution(_) => "subpar_duplicate_resolution",
        ActiveView::InboxCorpusMatchResolution(_) => "inbox_corpus_match_resolution",
        ActiveView::InboxOrganize(_) => "inbox_organize",
        ActiveView::DirectoryClusterResolution(_) => "directory_cluster_resolution",
        ActiveView::MovedFileAcknowledge(_) => "moved_file_acknowledge",
        ActiveView::OobSyncResolution(_) => "oob_sync_resolution",
        ActiveView::OobConflictInspection(_) => "oob_conflict_inspection",
        ActiveView::TagCanonicityResolution { .. } => "tag_canonicity_resolution",
        ActiveView::CompoundTagSplit { .. } => "compound_tag_split",
        ActiveView::MissingAlbumSingleResolution(_) => "missing_album_single",
        ActiveView::ManualReview(_) => "manual_review",
        ActiveView::TransactionReview(_) => "transaction_review",
    }
}
