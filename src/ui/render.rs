//! Main rendering functions for the TUI.
//!
//! This module contains all top-level render functions that dispatch
//! to mode-specific renderers.

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};
use std::collections::VecDeque;
use std::time::Instant;

use crate::config::Config;
use crate::flows::background::BackgroundTask;

use super::app::{EyeAnimation, EyeFrame, EYE_CLOSED, EYE_CLOSING, EYE_OPEN};
use super::helpers::{calculate_rolling_throughput, format_bytes_binary, format_duration, format_eta, truncate_path_display};
use super::widgets::{control_presets, Modal, ModalButton, ModalStyle};
use super::{deploy_flow, insights_view, tag_editor, tree_browser};

/// Display context passed to rendering functions.
/// Contains all the state needed to render the UI.
pub struct RenderContext<'a> {
    pub mode: super::UiMode,
    pub config: &'a Config,
    pub status_message: Option<&'a str>,
    pub tag_editor: Option<&'a mut tag_editor::TagEditorState>,
    pub tag_editor_modal: Option<&'a tag_editor::TagEditorModal>,
    pub tree_browser: Option<&'a mut tree_browser::TreeBrowserState>,
    pub drop_missing_state: Option<&'a super::DropMissingState>,
    pub deployment_preview: Option<&'a mut deploy_flow::DeploymentPreviewState>,
    pub unified_tag_editor: Option<&'a mut tag_editor::UnifiedTagEditorState>,
    pub exit_confirm_modal_state: Option<&'a super::ExitConfirmModalState>,
    pub splash_screen: Option<&'a super::splash_screen::SplashScreen>,
    pub deploy_conflict_review: Option<&'a super::DeployConflictReviewState>,
    pub insights_view: Option<&'a mut insights_view::InsightsViewState>,
    pub eye: &'a EyeAnimation,
    pub throughput_samples: &'a VecDeque<(Instant, u64)>,
    pub background_tasks: &'a [BackgroundTask],
    pub daemon_status: Option<crate::flows::DaemonStatus>,
}

/// Main render entry point - dispatches to sub-renderers based on mode.
pub fn render(f: &mut Frame, ctx: &mut RenderContext) {
    // Loading splash takes the whole screen
    if ctx.mode == super::UiMode::LoadingSplash {
        if let Some(splash) = ctx.splash_screen {
            super::splash_screen::render(f, f.area(), splash);
        }
        return;
    }

    // Lateral views (Corpus Browser, Insights, Deploy) have their own title bar
    // and get the full header+content area
    let uses_unified_titlebar = matches!(
        ctx.mode,
        super::UiMode::CorpusBrowser
            | super::UiMode::Insights
            | super::UiMode::DeploymentPreview
    );

    if uses_unified_titlebar {
        // Two-part layout: content (with unified titlebar) + footer
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(10),    // Content with unified titlebar
                Constraint::Length(18), // Footer with eye
            ])
            .split(f.area());

        render_content(f, chunks[0], ctx);
        render_footer(f, chunks[1], ctx);
    } else {
        // Standard three-part layout: header + content + footer
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),  // Header
                Constraint::Min(10),    // Content
                Constraint::Length(18), // Footer with eye
            ])
            .split(f.area());

        render_header(f, chunks[0], ctx);
        render_content(f, chunks[1], ctx);
        render_footer(f, chunks[2], ctx);
    }
}

fn render_header(f: &mut Frame, area: ratatui::layout::Rect, ctx: &RenderContext) {
    // Get mode-specific suffix (if any)
    let suffix = match ctx.mode {
        super::UiMode::TagEditor => Some("Tag Editor"),
        super::UiMode::DirBrowser => Some("Directory Browser"),
        super::UiMode::DropMissingConfirmation => Some("Drop Missing From Index"),
        super::UiMode::DeploymentPreview => Some("Deployment Preview"),
        super::UiMode::ExitConfirmModal => Some("Exit Confirmation"),
        super::UiMode::CorpusBrowser => Some("Corpus Browser"),
        super::UiMode::DeployConflictReview => Some("Deploy Conflict Review"),
        super::UiMode::Insights => Some("Corpus Insights"),
        super::UiMode::LoadingSplash => None, // Never reached - handled separately
        super::UiMode::UnifiedTagEditor => Some("Tag Editor"),
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
    match ctx.mode {
        super::UiMode::TagEditor => {
            if let Some(ref mut editor) = ctx.tag_editor {
                editor.render(f, area, ctx.status_message);
            }
            // Render modal on top if present
            if let Some(ref modal) = ctx.tag_editor_modal {
                match modal {
                    tag_editor::TagEditorModal::SaveConfirmation { selected_button } => {
                        tag_editor::render_save_confirmation_modal(f, area, *selected_button);
                    }
                    tag_editor::TagEditorModal::ChangePreview {
                        grouped_changes,
                        single_changes,
                        scroll_offset,
                        ..
                    } => {
                        tag_editor::render_change_preview_modal(
                            f,
                            area,
                            grouped_changes,
                            single_changes,
                            *scroll_offset,
                        );
                    }
                }
            }
        }
        super::UiMode::DirBrowser => {
            if let Some(ref mut browser) = ctx.tree_browser {
                browser.render(f, area);
            }
        }
        super::UiMode::DropMissingConfirmation => {
            render_drop_missing_confirmation(f, area, ctx);
        }
        super::UiMode::DeploymentPreview => {
            if let Some(ref mut preview) = ctx.deployment_preview {
                preview.render(f, area);
            }
        }
        super::UiMode::ExitConfirmModal => {
            render_exit_confirm_modal(f, area, ctx.exit_confirm_modal_state);
        }
        super::UiMode::CorpusBrowser => {
            if let Some(ref mut browser) = ctx.tree_browser {
                browser.render(f, area);
            }
        }
        super::UiMode::DeployConflictReview => {
            if let Some(ref review) = ctx.deploy_conflict_review {
                render_deploy_conflict_review(f, area, review, ctx.status_message);
            }
        }
        super::UiMode::Insights => {
            if let Some(ref mut view) = ctx.insights_view {
                insights_view::render_insights_view(f, area, view);
            } else {
                // Show loading state while insights view is initializing
                render_insights_loading(f, area);
            }
        }
        super::UiMode::LoadingSplash => {
            // Never reached - handled separately in render() before this function
        }
        super::UiMode::UnifiedTagEditor => {
            if let Some(ref mut editor) = ctx.unified_tag_editor {
                editor.render(f, area, ctx.status_message);
            }
        }
    }
}

// render_loading_splash has been moved to splash_screen::render()

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

/// Render the deploy conflict review screen
fn render_deploy_conflict_review(
    f: &mut Frame,
    area: ratatui::layout::Rect,
    review: &super::DeployConflictReviewState,
    status_message: Option<&str>,
) {
    

    // Layout: content area | buttons | status
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(10),    // Group list
            Constraint::Length(3),  // Buttons
            Constraint::Length(3),  // Status
        ])
        .split(area);

    // Render group list
    let total_tracks: usize = review.groups.iter().map(|g| g.track_count).sum();
    let total_decisions: usize = review.groups.iter().map(|g| g.decisions.len()).sum();

    let mut items: Vec<ListItem> = Vec::new();
    for (idx, group) in review.groups.iter().enumerate() {
        let decision_count = group.decisions.len();
        let line = if decision_count > 0 {
            format!(
                "Group {}: {} tracks → {} edits  [{}]",
                idx + 1,
                group.track_count,
                decision_count,
                truncate_path_display(&group.target_path, 40)
            )
        } else {
            format!(
                "Group {}: {} tracks → (no edits)  [{}]",
                idx + 1,
                group.track_count,
                truncate_path_display(&group.target_path, 40)
            )
        };
        items.push(ListItem::new(line));
    }

    let summary = format!(
        "Deploy Conflict Resolution - {} groups, {} tracks, {} total edits",
        review.groups.len(),
        total_tracks,
        total_decisions
    );

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(summary))
        .style(Style::default().fg(Color::White));
    f.render_widget(list, chunks[0]);

    // Render buttons
    let commit_style = if review.selected_button == 0 {
        Style::default().fg(Color::Black).bg(Color::Green).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Green)
    };
    let discard_style = if review.selected_button == 1 {
        Style::default().fg(Color::Black).bg(Color::Red).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Red)
    };

    let button_line = Line::from(vec![
        Span::raw("  "),
        Span::styled(" Commit ", commit_style),
        Span::raw("   "),
        Span::styled(" Discard ", discard_style),
        Span::raw("  "),
    ]);

    let buttons = Paragraph::new(button_line)
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL).title("Actions"));
    f.render_widget(buttons, chunks[1]);

    // Render status
    let status_text = status_message.unwrap_or("←/→ Select | Enter Confirm | Esc Cancel");
    let status = Paragraph::new(status_text)
        .style(Style::default().fg(Color::Yellow))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL).title("Status"));
    f.render_widget(status, chunks[2]);
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
            .size(50, 35)
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
            .size(35, 20)
            .style(ModalStyle::info())
            .centered()
            .render(f, area);
    }
}

pub fn render_drop_missing_confirmation(f: &mut Frame, area: ratatui::layout::Rect, ctx: &RenderContext) {
    if let Some(ref state) = ctx.drop_missing_state {
        let count = state.missing_tracks.len();

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),  // Header with count
                Constraint::Min(5),     // File list
                Constraint::Length(3),  // Action buttons
            ])
            .split(area);

        // Header
        let header_text = format!(
            "Found {} missing file{} in corpus index\nThese files no longer exist on disk but are still in the database.",
            count,
            if count == 1 { "" } else { "s" }
        );
        let header = Paragraph::new(header_text)
            .style(Style::default().fg(Color::Yellow))
            .block(Block::default().borders(Borders::ALL));
        f.render_widget(header, chunks[0]);

        // File list
        let max_visible = chunks[1].height.saturating_sub(2) as usize;
        let items: Vec<ListItem> = state
            .missing_tracks
            .iter()
            .skip(state.list_offset)
            .take(max_visible)
            .map(|track| {
                ListItem::new(track.path.clone())
                    .style(Style::default().fg(Color::White))
            })
            .collect();

        let list_title = format!(
            "Missing Files ({}-{} of {})",
            state.list_offset + 1,
            (state.list_offset + items.len()).min(count),
            count
        );
        let list = List::new(items)
            .block(Block::default().borders(Borders::ALL).title(list_title));
        f.render_widget(list, chunks[1]);

        // Action buttons
        let cancel_style = if state.selected_option == 0 {
            Style::default().fg(Color::Black).bg(Color::White)
        } else {
            Style::default().fg(Color::White)
        };
        let drop_style = if state.selected_option == 1 {
            Style::default().fg(Color::Black).bg(Color::Red)
        } else {
            Style::default().fg(Color::Red)
        };

        let buttons = Line::from(vec![
            Span::raw("  "),
            Span::styled(" Cancel ", cancel_style),
            Span::raw("    "),
            Span::styled(format!(" Drop {} entries ", count), drop_style),
            Span::raw("  "),
        ]);
        let buttons_para = Paragraph::new(buttons)
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL).title("Action"));
        f.render_widget(buttons_para, chunks[2]);
    }
}

fn render_footer(f: &mut Frame, area: ratatui::layout::Rect, ctx: &RenderContext) {
    let footer_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(30),      // Left status area (flexible)
            Constraint::Length(70),   // Eye animation (fixed 70 cols)
        ])
        .split(area);

    // Left side: three stacked status boxes
    let status_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),  // Corpus status
            Constraint::Length(6),  // Operation status
            Constraint::Min(4),     // Controls
        ])
        .split(footer_layout[0]);

    render_corpus_status(f, status_layout[0], ctx);
    render_operation_status(f, status_layout[1], ctx);
    render_controls(f, status_layout[2], ctx);

    // Eye animation
    render_eye(f, footer_layout[1], ctx);
}

fn render_corpus_status(f: &mut Frame, area: ratatui::layout::Rect, _ctx: &RenderContext) {
    // TODO: Corpus status should show health_issues summary from eyeballing
    let lines = vec![
        Line::from("No observation data")
            .style(Style::default().fg(Color::DarkGray)),
    ];

    let para = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title("Corpus"));
    f.render_widget(para, area);
}

fn render_operation_status(f: &mut Frame, area: ratatui::layout::Rect, ctx: &RenderContext) {
    let mut lines = Vec::new();

    let total_tasks = ctx.background_tasks.len();
    let has_daemon_work = ctx.daemon_status.as_ref().map(|s| s.pending > 0).unwrap_or(false);
    let has_completed_session = ctx.daemon_status.as_ref()
        .and_then(|s| s.completed_session.as_ref())
        .is_some();

    if total_tasks == 0 && !has_daemon_work && !has_completed_session {
        lines.push(
            Line::from("No operation in progress").style(Style::default().fg(Color::DarkGray)),
        );
    } else if has_daemon_work {
        // Show active daemon status
        if let Some(ref status) = ctx.daemon_status {
            // Build task type summary
            let task_summary: String = if !status.task_counts.is_empty() {
                status.task_counts.iter()
                    .map(|(k, v)| format!("{}: {}", k, v))
                    .collect::<Vec<_>>()
                    .join(", ")
            } else {
                "Processing...".to_string()
            };

            lines.push(Line::from(vec![
                Span::styled("Task Daemon ", Style::default().fg(Color::Cyan)),
                Span::styled(task_summary, Style::default().fg(Color::White)),
            ]));

            // Show elapsed time
            let elapsed_str = status.elapsed.map(|d| format_duration(d)).unwrap_or_default();
            lines.push(Line::from(format!(
                "Pending: {} | Processed: {}{}",
                status.pending,
                status.total_processed,
                if !elapsed_str.is_empty() { format!(" | {}", elapsed_str) } else { String::new() }
            )));

            if !status.recent_errors.is_empty() {
                lines.push(Line::from(
                    status.recent_errors.last().unwrap_or(&String::new()).clone()
                ).style(Style::default().fg(Color::Red)));
            }
        }
    } else if has_completed_session {
        // Show lingering completed session summary
        if let Some(ref status) = ctx.daemon_status {
            if let Some(ref session) = status.completed_session {
                // Build task type breakdown
                let task_breakdown: String = session.task_counts.iter()
                    .map(|(k, v)| format!("{}: {}", k, v))
                    .collect::<Vec<_>>()
                    .join(", ");

                let status_color = if session.failed > 0 { Color::Yellow } else { Color::Green };
                lines.push(Line::from(vec![
                    Span::styled("Completed ", Style::default().fg(status_color)),
                    Span::styled(task_breakdown, Style::default().fg(Color::White)),
                ]));

                let duration_str = format_duration(session.duration);
                let failed_str = if session.failed > 0 {
                    format!(" | {} failed", session.failed)
                } else {
                    String::new()
                };
                lines.push(Line::from(format!(
                    "{} tasks in {}{}",
                    session.total_processed,
                    duration_str,
                    failed_str
                )).style(Style::default().fg(Color::DarkGray)));
            }
        }
    } else {
        // Render background tasks
        for (i, task) in ctx.background_tasks.iter().enumerate() {
            let progress = &task.progress;

            // Task description with ETA
            let eta_span = if i == 0 {
                if let Some((processed, total)) = progress.bytes {
                    if processed > 0 && total > 0 {
                        let throughput = calculate_rolling_throughput(ctx.throughput_samples, 8);
                        if let Some(mib_per_sec) = throughput {
                            if mib_per_sec > 0.01 {
                                let remaining_bytes = total.saturating_sub(processed);
                                let remaining_mib = remaining_bytes as f64 / (1024.0 * 1024.0);
                                let eta_secs = (remaining_mib / mib_per_sec) as u64;
                                Span::styled(
                                    format!(" (ETA: {})", format_eta(eta_secs)),
                                    Style::default().fg(Color::DarkGray),
                                )
                            } else {
                                Span::raw("")
                            }
                        } else {
                            Span::raw("")
                        }
                    } else {
                        Span::raw("")
                    }
                } else {
                    Span::raw("")
                }
            } else {
                Span::raw("")
            };

            lines.push(Line::from(vec![
                Span::styled(&task.label, Style::default().fg(Color::Cyan)),
                eta_span,
            ]));

            // Items progress
            if total_tasks > 1 {
                lines.push(Line::from(format!(
                    "  {}/{} ({} skipped)",
                    progress.completed,
                    progress.total,
                    progress.skipped
                )));
            } else {
                lines.push(Line::from(format!(
                    "Items: {}/{} | Skipped: {} unchanged",
                    progress.completed,
                    progress.total,
                    progress.skipped
                )));

                // Bytes progress
                if let Some((processed, total)) = progress.bytes {
                    let processed_str = format_bytes_binary(processed);
                    let total_str = format_bytes_binary(total);
                    let throughput_str = match calculate_rolling_throughput(ctx.throughput_samples, 8) {
                        Some(mib_per_sec) => format!(" ({:.1} MiB/s)", mib_per_sec),
                        None => String::new(),
                    };
                    lines.push(Line::from(format!(
                        "Bytes: {} / {}{}",
                        processed_str, total_str, throughput_str
                    )));
                }

                // Current item
                if let Some(ref item) = progress.current_item {
                    let display = truncate_path_display(item, 50);
                    lines.push(Line::from(display).style(Style::default().fg(Color::DarkGray)));
                }
            }
        }
    }

    let title = if total_tasks > 1 {
        format!("Tasks ({})", total_tasks)
    } else {
        "Task".to_string()
    };

    let para = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(title));
    f.render_widget(para, area);
}

fn render_controls(f: &mut Frame, area: ratatui::layout::Rect, ctx: &RenderContext) {
    let mut lines = Vec::new();

    // Show any status message first
    if let Some(msg) = ctx.status_message {
        lines.push(Line::from(msg.to_string()).style(Style::default().fg(Color::Yellow)));
    }

    // Get mode-specific controls hint
    let controls = match ctx.mode {
        super::UiMode::TagEditor => control_presets::tag_editor(),
        super::UiMode::DirBrowser => control_presets::dir_browser(),
        super::UiMode::DropMissingConfirmation => control_presets::drop_missing(),
        super::UiMode::DeploymentPreview => control_presets::deployment_preview(),
        super::UiMode::ExitConfirmModal => control_presets::exit_confirm_modal(),
        super::UiMode::CorpusBrowser => control_presets::corpus_browser(),
        super::UiMode::DeployConflictReview => control_presets::deploy_conflict_review(),
        super::UiMode::Insights => control_presets::insights_view(),
        super::UiMode::LoadingSplash => control_presets::empty(),
        super::UiMode::UnifiedTagEditor => control_presets::tag_editor(), // Reuse same controls
    };
    lines.push(controls.render_line());

    let para = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title("Controls"));
    f.render_widget(para, area);
}

fn render_eye(f: &mut Frame, area: ratatui::layout::Rect, ctx: &RenderContext) {
    let eye_text = match ctx.eye.current_frame() {
        EyeFrame::Open => EYE_OPEN,
        EyeFrame::Closing => EYE_CLOSING,
        EyeFrame::Closed => EYE_CLOSED,
    };

    // Center the 64-col eye in the 70-col frame
    let centered_eye: String = eye_text
        .lines()
        .map(|line| format!("   {}   ", line))
        .collect::<Vec<_>>()
        .join("\n");

    let eye_para = Paragraph::new(centered_eye)
        .style(Style::default().fg(Color::Cyan))
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(eye_para, area);
}
