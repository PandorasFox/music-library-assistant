//! Main rendering functions for the TUI.
//!
//! This module contains all top-level render functions that dispatch
//! to view-specific renderers based on the ActiveView enum.

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
use super::widgets::{status_bar, Modal, ModalButton, ModalFrame, ModalStyle, UnifiedTitleBar};
use super::{
    compound_split_v2, config_editor, external_match_modal, inbox_view,
    insights_view, manual_review_modal, oob_conflict_modal, oob_sync_modal, progressive_worker,
    tabbed_transaction_review, tag_canonicity_v2, transaction_review,
};

/// Main render entry point - dispatches to sub-renderers based on ActiveView.
pub(crate) fn render_app(
    f: &mut Frame,
    app: &mut super::App,
    status_line_1: Option<String>,
    status_line_2: Option<String>,
) {
    // Progress screen takes the whole screen (startup, content analysis, signal refresh)
    if let ActiveView::Progress {
        ref screen,
        ref eye,
    } = app.view
    {
        // For non-eyeballing phases, provide animated eye frame
        let eye_frame = if screen.uses_closed_eye() {
            None // Uses its own closed/derivation eye
        } else {
            // Awake phase - use animated eye
            Some(match eye.current_frame() {
                EyeFrame::Open => EYE_OPEN,
                EyeFrame::Closing => EYE_CLOSING,
                EyeFrame::Closed => EYE_CLOSED,
            })
        };
        super::progress_screen::render(f, f.area(), screen, eye_frame);
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

        // Read deploy_needs_action from locally cached deploy status
        let deploy_needs_action = app.query(mm_meta::domain_queries::GetDeployStatus).needs_action;

        let transactions_open = app.config().opinions.leave_transactions_open;
        let transaction_has_decisions = {
            app.witch_status().transaction.as_ref()
                .is_some_and(|t| t.decision_count > 0)
        };

        let titlebar = UnifiedTitleBar::new(lv)
            .with_deploy_needs_action(deploy_needs_action)
            .with_transactions_open(transactions_open)
            .with_transaction_has_decisions(transaction_has_decisions);
        app.tab_click_rects = titlebar.tab_click_rects(chunks[0]);
        titlebar.render(f, chunks[0]);

        render_content(f, app, chunks[1]);
        render_status_bar(
            f,
            chunks[2],
            status_line_1.as_deref(),
            status_line_2.as_deref(),
        );
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

        render_header(f, chunks[0], &app.view);
        render_content(f, app, chunks[1]);
        render_status_bar(
            f,
            chunks[2],
            status_line_1.as_deref(),
            status_line_2.as_deref(),
        );
    }
}

fn render_header(f: &mut Frame, area: ratatui::layout::Rect, view: &ActiveView) {
    let suffix = view.header_suffix();

    let title = match suffix {
        Some(s) => format!("{} - {}", mm_meta::MM_TITLE, s),
        None => mm_meta::MM_TITLE.to_string(),
    };

    let header = Paragraph::new(title)
        .style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));

    f.render_widget(header, area);
}

fn render_content(f: &mut Frame, app: &mut super::App, area: ratatui::layout::Rect) {
    match app.view {
        ActiveView::StartupMaintenance => {
            startup::render_startup_maintenance(f, area, app);
        }
        ActiveView::Progress { .. } => {
            // Never reached - handled separately in render_app() before this function
        }
        ActiveView::ConfigEditor(ref mut state) => {
            config_editor::render::render(f, area, state);
        }
        ActiveView::ProgressiveWork(ref worker) => {
            progressive_worker::render(f, area, worker);
        }
        ActiveView::Deploy(ref state) => {
            state.render(f, area);
        }
        ActiveView::ExitConfirm(ref mut state) => {
            render_exit_confirm_modal(f, area, state);
        }
        ActiveView::CorpusBrowser(ref mut browser) => {
            browser.render(f, area, &mut app.art_picker, &mut app.art_cache);
        }
        ActiveView::Insights(ref mut view) => {
            insights_view::render_insights_view(f, area, view);
        }
        ActiveView::History(ref mut state) => {
            super::history_view::render::render(f, area, state);
        }
        ActiveView::ExternalMatches(ref mut state) => {
            super::external_match_view::render::render(f, area, state);
        }
        ActiveView::Inbox(ref mut state) => {
            inbox_view::render_inbox_view(f, area, state);
        }
        ActiveView::TabbedTransactionReview(ref mut state) => {
            tabbed_transaction_review::render(f, area, state);
        }
        ActiveView::TagSearch(ref mut state) => {
            state.render(f, area);
        }
        ActiveView::IntakeConfirmation(ref state) => {
            super::startup::intake_confirmation::render(f, area, state);
        }
        ActiveView::UnifiedTagEditor(ref mut editor) => {
            editor.render(f, area, &mut app.art_picker, &mut app.art_cache, &app.resolver);
        }
        ActiveView::MissingFileResolution(ref mut preview) => {
            preview.render(f, area);
        }
        ActiveView::MissingDirectoryResolution(ref mut preview) => {
            preview.render(f, area);
        }
        ActiveView::TagCanonicityResolution { ref mut state, .. } => {
            tag_canonicity_v2::render(f, area, state);
        }
        ActiveView::CompoundTagSplit { ref mut state, .. } => {
            compound_split_v2::render(f, area, state);
        }
        ActiveView::OobSyncResolution(ref mut state) => {
            oob_sync_modal::render(f, area, state);
        }
        ActiveView::OobConflictInspection(ref mut state) => {
            oob_conflict_modal::render(f, area, state);
        }
        ActiveView::ExternalMatchReview(ref mut state) => {
            external_match_modal::render(f, area, state);
        }
        ActiveView::ReleasePackingBrowser(ref mut state) => {
            super::release_packing_browser::render::render(f, area, state);
        }
        ActiveView::KnotBrowser(ref mut state) => {
            super::knot_browser::render::render(f, area, state);
        }
        ActiveView::MovedFileAcknowledge(ref mut state) => {
            super::moved_file_modal::render(state, f, area);
        }
        ActiveView::TransactionReview(ref mut review) => {
            transaction_review::render(f, area, review);
        }
        ActiveView::CorruptFileResolution(ref mut preview) => {
            preview.render_frame(f, area);
        }
        ActiveView::ShitFormatResolution(ref mut preview) => {
            preview.render(f, area);
        }
        ActiveView::SubparDuplicateResolution(ref mut preview) => {
            preview.render(f, area);
        }
        ActiveView::InboxCorpusMatchResolution(ref mut preview) => {
            preview.render(f, area);
        }
        ActiveView::InboxOrganize(ref mut state) => {
            super::inbox_organize::render::render(f, area, state);
        }
        ActiveView::DirectoryClusterResolution(ref mut preview) => {
            preview.render(f, area);
        }
        ActiveView::MissingAlbumSingleResolution(ref mut state) => {
            state.render(f, area);
        }
        ActiveView::DiscExtractionResolution(ref mut state) => {
            state.render(f, area);
        }
        ActiveView::ManualReview(ref state) => {
            manual_review_modal::render(f, area, state);
        }
    }

}

fn render_exit_confirm_modal(
    f: &mut Frame,
    area: ratatui::layout::Rect,
    state: &mut ExitConfirmModalState,
) {
    let selected = state.selected;
    let has_operations = state.has_operations;

    // Build top-row buttons with appropriate styles
    let (yes_btn, no_btn) = if has_operations {
        // Warning modal: Yes is red (dangerous), No is green (safe)
        let mut yes_btn = ModalButton::new("Yes", "").with_indicator().styles(
            Style::default().fg(Color::Black).bg(Color::Red),
            Style::default().fg(Color::White),
        );
        let mut no_btn = ModalButton::new("No", "").with_indicator().styles(
            Style::default().fg(Color::Black).bg(Color::Green),
            Style::default().fg(Color::White),
        );
        if selected == 0 { yes_btn = yes_btn.selected(); }
        if selected == 1 { no_btn = no_btn.selected(); }
        (yes_btn, no_btn)
    } else {
        // Simple exit: neutral styles
        let mut confirm_btn = ModalButton::new("Confirm", "").with_indicator().styles(
            Style::default().fg(Color::Black).bg(Color::Cyan),
            Style::default().fg(Color::White),
        );
        let mut cancel_btn = ModalButton::new("Cancel", "").with_indicator().styles(
            Style::default().fg(Color::Black).bg(Color::Gray),
            Style::default().fg(Color::White),
        );
        if selected == 0 { confirm_btn = confirm_btn.selected(); }
        if selected == 1 { cancel_btn = cancel_btn.selected(); }
        (confirm_btn, cancel_btn)
    };

    // Shutdown button (always present)
    let mut shutdown_btn = ModalButton::new("Yes, and send server shutdown", "").with_indicator().styles(
        Style::default().fg(Color::Black).bg(Color::Red),
        Style::default().fg(Color::DarkGray),
    );
    if selected == 2 { shutdown_btn = shutdown_btn.selected(); }

    // Build button lines
    let mut button_spans = yes_btn.render_with_indicator();
    button_spans.push(Span::raw("     "));
    button_spans.extend(no_btn.render_with_indicator());
    let button_line = Line::from(button_spans);
    let shutdown_line = Line::from(shutdown_btn.render_with_indicator());

    // Compute button click targets
    let (modal_w, modal_h, button_line_idx) = if has_operations {
        (50u16, 18u16, 12u16)
    } else {
        (40u16, 11u16, 4u16)
    };
    let modal_rect = super::widgets::centered_rect_fixed(modal_w, modal_h, area);
    let inner_x = modal_rect.x + 1; // inside Borders::ALL
    let inner_y = modal_rect.y + 1;
    let button_row = inner_y + button_line_idx;
    let shutdown_row = button_row + 2; // blank line between

    state.button_rects.clear();
    let mut bx = inner_x;
    // First button: " > " (3 chars) + label
    let first_label_len = if has_operations { 3u16 } else { 7u16 }; // "Yes" / "Confirm"
    let first_width = 3 + first_label_len;
    state.button_rects.set(
        "yes",
        ratatui::layout::Rect::new(bx, button_row, first_width, 1),
    );
    bx += first_width + 5; // 5-char gap
    // Second button: " > " (3 chars) + label
    let second_label_len = if has_operations { 2u16 } else { 6u16 }; // "No" / "Cancel"
    let second_width = 3 + second_label_len;
    state.button_rects.set(
        "no",
        ratatui::layout::Rect::new(bx, button_row, second_width, 1),
    );
    // Shutdown button: " > " (3 chars) + label
    let shutdown_width = 3 + 30u16; // "Yes, and send server shutdown"
    state.button_rects.set(
        "shutdown",
        ratatui::layout::Rect::new(inner_x, shutdown_row, shutdown_width, 1),
    );

    if has_operations {
        // Warning modal for operations in progress
        let content = vec![
            Line::from(""),
            Line::from(Span::styled(
                "Operation In Progress",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
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
            Line::from(""),
            shutdown_line,
        ];

        Modal::new()
            .title(" Warning ")
            .content(content)
            .fixed_size(50, 18)
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
            Line::from(""),
            shutdown_line,
        ];

        Modal::new()
            .title(" Exit ")
            .content(content)
            .fixed_size(40, 11)
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

