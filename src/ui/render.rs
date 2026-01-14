//! Main rendering functions for the TUI.
//!
//! This module contains all top-level render functions that dispatch
//! to mode-specific renderers.

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
    Frame,
};
use std::collections::VecDeque;
use std::time::Instant;

use crate::config::Config;
use crate::corpus::HeartbeatResult;
use crate::ops::operation::{OperationProgress, OperationType, ProgressContext};

use super::app::{EyeAnimation, EyeFrame, EYE_CLOSED, EYE_CLOSING, EYE_OPEN};
use super::helpers::{calculate_rolling_throughput, centered_rect, format_bytes_binary, format_eta, truncate_path_display};
use super::main_menu::MainMenuState;
use super::{album_artist_flow, album_flow, canon_flow, dedup_flow, deploy_flow, dialogue, dir_browser, tag_editor};

/// Display context passed to rendering functions.
/// Contains all the state needed to render the UI.
pub struct RenderContext<'a> {
    pub mode: super::UiMode,
    pub config: &'a Config,
    pub status_message: Option<&'a str>,
    pub main_menu: &'a mut MainMenuState,
    pub tag_editor: Option<&'a mut tag_editor::TagEditorState>,
    pub tag_editor_modal: Option<&'a tag_editor::TagEditorModal>,
    pub dir_browser: Option<&'a mut dir_browser::DirBrowserState>,
    pub corpus_browser: Option<&'a mut super::corpus_browser::CorpusBrowserState>,
    pub dialogue: Option<&'a mut dialogue::DialogueState>,
    pub dialogue_summary: Option<&'a mut dialogue::DialogueSummaryState>,
    pub cluster_dialogue: Option<&'a mut dedup_flow::ClusterDialogueState>,
    pub bulk_prompt: Option<&'a mut dedup_flow::BulkPromptState>,
    pub session_review: Option<&'a mut dedup_flow::SessionReviewState>,
    pub drop_missing_state: Option<&'a super::DropMissingState>,
    pub deployment_preview: Option<&'a mut deploy_flow::DeploymentPreviewState>,
    pub canon_cluster_view: Option<&'a mut canon_flow::ClusterViewState>,
    pub canon_session_review: Option<&'a mut canon_flow::ReviewState>,
    pub canon_commit_modal_state: Option<&'a super::CanonCommitModalState>,
    pub album_artist_phase_selector: Option<&'a album_artist_flow::PhaseSelectorState>,
    pub album_artist_cluster_view: Option<&'a mut album_artist_flow::AlbumArtistClusterState>,
    pub album_artist_collation: Option<&'a mut album_artist_flow::CollationState>,
    pub album_artist_collation_review: Option<&'a mut album_artist_flow::CollationReviewState>,
    pub album_artist_population: Option<&'a mut album_artist_flow::PopulationState>,
    pub album_artist_population_review: Option<&'a mut album_artist_flow::PopulationReviewState>,
    pub album_artist_review: Option<&'a mut album_artist_flow::AlbumArtistReviewState>,
    pub album_cluster_view: Option<&'a mut album_flow::AlbumClusterState>,
    pub album_review: Option<&'a mut album_flow::AlbumReviewState>,
    pub directory_tag_editor: Option<&'a mut tag_editor::DirectoryTagEditorState>,
    pub directory_tag_editor_modal: Option<&'a tag_editor::types::DirectoryTagEditorModal>,
    pub exit_confirm_modal_state: Option<&'a super::ExitConfirmModalState>,
    pub deploy_conflict_review: Option<&'a super::DeployConflictReviewState>,
    pub heartbeat_result: Option<&'a HeartbeatResult>,
    pub heartbeat_pending: bool,
    pub eye: &'a EyeAnimation,
    pub throughput_samples: &'a VecDeque<(Instant, u64)>,
    pub active_operations: Vec<(OperationType, OperationProgress)>,
}

/// Main render entry point - dispatches to sub-renderers based on mode.
pub fn render(f: &mut Frame, ctx: &mut RenderContext) {
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

fn render_header(f: &mut Frame, area: ratatui::layout::Rect, ctx: &RenderContext) {
    // Get mode-specific suffix (if any)
    let suffix = match ctx.mode {
        super::UiMode::MainMenu => None,
        super::UiMode::TagEditor => Some("Tag Editor"),
        super::UiMode::DirBrowser => Some("Directory Browser"),
        super::UiMode::Dialogue => Some("Decision Flow"),
        super::UiMode::DialogueSummary => Some("Session Summary"),
        super::UiMode::ClusterDialogue => Some("Fingerprint Deduplication"),
        super::UiMode::BulkReviewPrompt => Some("Bulk Decision Point"),
        super::UiMode::SessionReview => Some("Session Review"),
        super::UiMode::DropMissingConfirmation => Some("Drop Missing From Index"),
        super::UiMode::DeploymentPreview => Some("Deployment Preview"),
        super::UiMode::CanonClusterView => Some("Artist Canonicalization"),
        super::UiMode::CanonSessionReview => Some("Artist Canonicalization Review"),
        super::UiMode::CanonCommitModal => Some("Artist Canonicalization"),
        super::UiMode::ExitConfirmModal => Some("Exit Confirmation"),
        super::UiMode::CorpusBrowser => Some("Corpus Browser"),
        super::UiMode::AlbumArtistPhaseSelector => Some("Album Artist Resolution"),
        super::UiMode::AlbumArtistClusterView => Some("Album Artist Canonicalization"),
        super::UiMode::AlbumArtistCollation => Some("Album Artist Collation"),
        super::UiMode::AlbumArtistCollationReview => Some("Album Artist Collation Review"),
        super::UiMode::AlbumArtistPopulation => Some("Album Artist Population"),
        super::UiMode::AlbumArtistPopulationReview => Some("Album Artist Population Review"),
        super::UiMode::AlbumArtistReview => Some("Album Artist Review"),
        super::UiMode::AlbumClusterView => Some("Album Tag Resolution"),
        super::UiMode::AlbumReview => Some("Album Tag Review"),
        super::UiMode::DirectoryTagEditor => Some("Directory Tag Editor"),
        super::UiMode::DeployConflictReview => Some("Deploy Conflict Review"),
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
        super::UiMode::MainMenu => {
            ctx.main_menu.render(f, area, &Some(ctx.config.clone()));
        }
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
            if let Some(ref mut browser) = ctx.dir_browser {
                browser.render(f, area);
            }
        }
        super::UiMode::Dialogue => {
            if let Some(ref mut dialogue) = ctx.dialogue {
                dialogue.render(f, area);
            }
        }
        super::UiMode::DialogueSummary => {
            if let Some(ref mut summary) = ctx.dialogue_summary {
                summary.render(f, area);
            }
        }
        super::UiMode::ClusterDialogue => {
            if let Some(ref mut cluster_dlg) = ctx.cluster_dialogue {
                cluster_dlg.render(f, area);
            }
        }
        super::UiMode::BulkReviewPrompt => {
            if let Some(ref mut bulk_prompt) = ctx.bulk_prompt {
                bulk_prompt.render(f, area);
            }
        }
        super::UiMode::SessionReview => {
            if let Some(ref mut session_review) = ctx.session_review {
                session_review.render(f, area);
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
        super::UiMode::CanonClusterView => {
            if let Some(ref mut cluster_view) = ctx.canon_cluster_view {
                cluster_view.render(f, area);
            }
        }
        super::UiMode::CanonSessionReview => {
            if let Some(ref mut review) = ctx.canon_session_review {
                review.render(f, area);
            }
        }
        super::UiMode::CanonCommitModal => {
            render_canon_commit_modal(f, area, ctx.canon_commit_modal_state);
        }
        super::UiMode::ExitConfirmModal => {
            render_exit_confirm_modal(f, area, ctx.exit_confirm_modal_state);
        }
        super::UiMode::CorpusBrowser => {
            if let Some(ref mut browser) = ctx.corpus_browser {
                browser.render(f, area);
            }
        }
        super::UiMode::AlbumArtistPhaseSelector => {
            // Render main menu as background
            ctx.main_menu.render(f, area, &Some(ctx.config.clone()));
            // Render phase selector popup on top
            if let Some(ref selector) = ctx.album_artist_phase_selector {
                album_artist_flow::render_phase_selector(f, selector);
            }
        }
        super::UiMode::AlbumArtistClusterView => {
            if let Some(ref mut cluster_view) = ctx.album_artist_cluster_view {
                cluster_view.render(f, area);
            }
        }
        super::UiMode::AlbumArtistReview => {
            if let Some(ref mut review) = ctx.album_artist_review {
                review.render(f, area);
            }
        }
        super::UiMode::AlbumArtistCollation => {
            if let Some(ref mut collation) = ctx.album_artist_collation {
                collation.render(f, area);
            }
        }
        super::UiMode::AlbumArtistCollationReview => {
            if let Some(ref mut review) = ctx.album_artist_collation_review {
                review.render(f, area);
            }
        }
        super::UiMode::AlbumArtistPopulation => {
            if let Some(ref mut population) = ctx.album_artist_population {
                population.render(f, area);
            }
        }
        super::UiMode::AlbumArtistPopulationReview => {
            if let Some(ref mut review) = ctx.album_artist_population_review {
                review.render(f, area);
            }
        }
        super::UiMode::AlbumClusterView => {
            if let Some(ref mut cluster_view) = ctx.album_cluster_view {
                cluster_view.render(f, area);
            }
        }
        super::UiMode::AlbumReview => {
            if let Some(ref mut review) = ctx.album_review {
                review.render(f, area);
            }
        }
        super::UiMode::DirectoryTagEditor => {
            if let Some(ref mut editor) = ctx.directory_tag_editor {
                editor.render(f, area, ctx.status_message);
            }
            // Render modal on top if present
            if let Some(ref modal) = ctx.directory_tag_editor_modal {
                use tag_editor::types::DirectoryTagEditorModal;
                match modal {
                    DirectoryTagEditorModal::ChangePreview { scroll_offset, .. } => {
                        if let Some(ref editor) = ctx.directory_tag_editor {
                            let changes = editor.compute_changes();
                            tag_editor::directory_render::render_directory_change_preview_modal(
                                f,
                                area,
                                &changes,
                                editor.files.len(),
                                *scroll_offset,
                            );
                        }
                    }
                    DirectoryTagEditorModal::UnsavedChanges { going_next } => {
                        tag_editor::directory_render::render_unsaved_changes_modal(
                            f,
                            area,
                            *going_next,
                        );
                    }
                }
            }
        }
        super::UiMode::DeployConflictReview => {
            if let Some(ref review) = ctx.deploy_conflict_review {
                render_deploy_conflict_review(f, area, review, ctx.status_message);
            }
        }
    }
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
    let total_changes: usize = review.groups.iter().map(|g| g.changes.len()).sum();

    let mut items: Vec<ListItem> = Vec::new();
    for (idx, group) in review.groups.iter().enumerate() {
        let change_count = group.changes.len();
        let line = if change_count > 0 {
            format!(
                "Group {}: {} tracks → {} edits  [{}]",
                idx + 1,
                group.track_count,
                change_count,
                truncate_path_display(&group.target_path, 40)
            )
        } else {
            format!(
                "Group {}: {} tracks → (no changes)  [{}]",
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
        total_changes
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
    let popup_area = centered_rect(50, 35, area);

    // Clear the area first to prevent bleed-through
    f.render_widget(Clear, popup_area);

    let selected_no = state.map(|s| s.selected_no).unwrap_or(true);

    // Button styles
    let yes_style = if !selected_no {
        Style::default().fg(Color::Black).bg(Color::Red)
    } else {
        Style::default().fg(Color::White)
    };
    let no_style = if selected_no {
        Style::default().fg(Color::Black).bg(Color::Green)
    } else {
        Style::default().fg(Color::White)
    };

    let lines = vec![
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
        Line::from(vec![
            Span::styled(
                if !selected_no { " > " } else { "   " },
                yes_style,
            ),
            Span::styled("[ Yes ]", yes_style),
            Span::raw("     "),
            Span::styled(
                if selected_no { " > " } else { "   " },
                no_style,
            ),
            Span::styled("[ No ]", no_style),
        ]),
    ];

    let modal = Paragraph::new(lines)
        .alignment(Alignment::Center)
        .style(Style::default().bg(Color::Black))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow))
                .style(Style::default().bg(Color::Black))
                .title(" Warning ")
                .title_style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        );

    f.render_widget(modal, popup_area);
}

pub fn render_canon_commit_modal(
    f: &mut Frame,
    area: ratatui::layout::Rect,
    state: Option<&super::CanonCommitModalState>,
) {
    let popup_area = centered_rect(60, 50, area);

    // Clear the area first to prevent bleed-through
    f.render_widget(Clear, popup_area);

    let (tracks_updated, stale_count, selected) = match state {
        Some(s) => (s.tracks_updated, s.stale_deployments, s.selected_option),
        None => (0, 0, 1),
    };

    // Option styles
    let option_0_style = if selected == 0 {
        Style::default().fg(Color::Black).bg(Color::Yellow)
    } else {
        Style::default().fg(Color::White)
    };
    let option_1_style = if selected == 1 {
        Style::default().fg(Color::Black).bg(Color::Yellow)
    } else {
        Style::default().fg(Color::White)
    };

    let mut lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "Tags committed to database.",
            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("Background operation started to flush"),
        Line::from("tags to disk."),
        Line::from(""),
        Line::from(format!("{} tracks updated in database", tracks_updated)),
    ];

    // Show stale count if any
    if stale_count > 0 {
        lines.push(Line::from(Span::styled(
            format!("{} deployments now stale", stale_count),
            Style::default().fg(Color::Yellow),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            "No stale deployments",
            Style::default().fg(Color::DarkGray),
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(""));

    // Option 0: Return to main menu
    let prefix_0 = if selected == 0 { "> " } else { "  " };
    lines.push(Line::from(Span::styled(
        format!("{}[ Return to Main Menu ]", prefix_0),
        option_0_style,
    )));

    lines.push(Line::from(""));

    // Option 1: Proceed to deployment review
    let prefix_1 = if selected == 1 { "> " } else { "  " };
    lines.push(Line::from(Span::styled(
        format!("{}[ Proceed to Deployment Review ]", prefix_1),
        option_1_style,
    )));

    let modal = Paragraph::new(lines)
        .alignment(Alignment::Center)
        .style(Style::default().bg(Color::Black))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Green))
                .style(Style::default().bg(Color::Black))
                .title("Complete"),
        );
    f.render_widget(modal, popup_area);
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

fn render_corpus_status(f: &mut Frame, area: ratatui::layout::Rect, ctx: &RenderContext) {
    let mut lines = Vec::new();

    // Show heartbeat status first
    if let Some(ref hb) = ctx.heartbeat_result {
        if hb.is_corpus_healthy() {
            lines.push(
                Line::from(format!("Validated ({:.0}ms)", hb.duration.as_millis()))
                    .style(Style::default().fg(Color::Green)),
            );
        } else {
            if hb.missing_from_disk > 0 {
                lines.push(
                    Line::from(format!("{} missing", hb.missing_from_disk))
                        .style(Style::default().fg(Color::Yellow)),
                );
            }
            if hb.new_on_disk > 0 {
                lines.push(
                    Line::from(format!("{} new files", hb.new_on_disk))
                        .style(Style::default().fg(Color::Cyan)),
                );
            }
        }
    } else if ctx.heartbeat_pending {
        lines.push(Line::from("Validating...").style(Style::default().fg(Color::DarkGray)));
    }

    // Show corpus summary from cached data
    if let Some(ref summary) = ctx.main_menu.corpus_summary {
        if summary.track_count == 0 {
            lines.push(
                Line::from("No scan data")
                    .style(Style::default().fg(Color::Yellow)),
            );
        } else {
            lines.push(Line::from(format!("Indexed: {} tracks", summary.track_count)));

            // Deployment status from heartbeat library health
            if let Some(ref hb) = ctx.heartbeat_result {
                if !hb.library_health.is_empty() {
                    let total_healthy: usize = hb.library_health.iter().map(|l| l.healthy).sum();
                    let total_not_deployed: usize = hb.library_health.iter().map(|l| l.not_deployed).sum();
                    let total_stale: usize = hb.library_health.iter().map(|l| l.stale).sum();
                    let total_orphans: usize = hb.library_health.iter().map(|l| l.orphans).sum();
                    let total_files = total_healthy + total_not_deployed;
                    let total_issues = total_not_deployed + total_stale + total_orphans;

                    if total_files > 0 {
                        let deploy_text = format!("{}/{} deployed", total_healthy, total_files);
                        if total_issues > 0 {
                            lines.push(
                                Line::from(format!("{} ({} issues)", deploy_text, total_issues))
                                    .style(Style::default().fg(Color::Yellow)),
                            );
                        } else {
                            lines.push(
                                Line::from(deploy_text)
                                    .style(Style::default().fg(Color::Green)),
                            );
                        }
                    }
                }
            } else if let Some(ref ds) = summary.deployment_stats {
                lines.push(Line::from(format!(
                    "Deployed: {:.0}%",
                    ds.deployment_percentage
                )));
            }

            // Deploy conflict status
            if summary.deploy_conflicts > 0 {
                lines.push(
                    Line::from(format!("Deploy conflicts: {}", summary.deploy_conflicts))
                        .style(Style::default().fg(Color::Yellow)),
                );
            }

            // Pending changes
            let pending_total: usize = summary.pending_changes.values().sum();
            if pending_total > 0 {
                lines.push(
                    Line::from(format!("Pending: {} changes", pending_total))
                        .style(Style::default().fg(Color::Cyan)),
                );
            }
        }
    }

    let para = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title("Corpus"));
    f.render_widget(para, area);
}

fn render_operation_status(f: &mut Frame, area: ratatui::layout::Rect, ctx: &RenderContext) {
    let mut lines = Vec::new();

    if ctx.active_operations.is_empty() {
        lines.push(
            Line::from("No operation in progress").style(Style::default().fg(Color::DarkGray)),
        );
    } else {
        for (i, (op_type, progress)) in ctx.active_operations.iter().enumerate() {
            // Operation description with ETA
            let desc = op_type.description();
            let eta_span = {
                let bytes_processed = progress.bytes_processed.unwrap_or(0);
                let total_bytes = progress.total_bytes.unwrap_or(0);
                if bytes_processed > 0 && total_bytes > 0 {
                    if i == 0 {
                        let throughput = calculate_rolling_throughput(ctx.throughput_samples, 8);
                        if let Some(mib_per_sec) = throughput {
                            if mib_per_sec > 0.01 {
                                let remaining_bytes = total_bytes.saturating_sub(bytes_processed);
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
            };

            lines.push(Line::from(vec![
                Span::styled(desc, Style::default().fg(Color::Cyan)),
                eta_span,
            ]));

            // Items progress
            if ctx.active_operations.len() > 1 {
                lines.push(Line::from(format!(
                    "  {}/{} ({} skipped)",
                    progress.completed_items,
                    progress.total_items,
                    progress.skipped_items
                )));
            } else {
                lines.push(Line::from(format!(
                    "Items: {}/{} | Skipped: {} unchanged",
                    progress.completed_items,
                    progress.total_items,
                    progress.skipped_items
                )));

                // Mtime mismatch statistics
                if let Some(ProgressContext::Scan { ref mtime_stats }) = progress.context {
                    if mtime_stats.mismatch_count > 0 || mtime_stats.not_in_db_count > 0 {
                        let mut parts = Vec::new();
                        if mtime_stats.not_in_db_count > 0 {
                            parts.push(format!("new:{}", mtime_stats.not_in_db_count));
                        }
                        if mtime_stats.mismatch_count > 0 {
                            parts.push(format!("mtime_delta:{}", mtime_stats.mismatch_count));
                            if let Some(mean) = mtime_stats.mean_diff_secs() {
                                let median = mtime_stats.median_diff_secs().unwrap_or(0);
                                let mode = mtime_stats.mode_diff_secs().unwrap_or(0);
                                let stddev = mtime_stats.stddev_diff_secs().unwrap_or(0.0);
                                parts.push(format!(
                                    "μ={:.1}s med={}s mode={}s σ={:.1}",
                                    mean, median, mode, stddev
                                ));
                            }
                        }
                        lines.push(
                            Line::from(parts.join(" | ")).style(Style::default().fg(Color::DarkGray)),
                        );
                    }
                }

                // Bytes progress
                if let (Some(processed), Some(total)) = (progress.bytes_processed, progress.total_bytes) {
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

    let title = if ctx.active_operations.len() > 1 {
        format!("Operations ({})", ctx.active_operations.len())
    } else {
        "Operation".to_string()
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

    // Navigation hints based on mode
    let hints = match ctx.mode {
        super::UiMode::MainMenu => "↑↓ Navigate | ←→ Pane | Enter Select | Q Quit",
        super::UiMode::TagEditor => "Tab Tracks | ↑↓ Fields | Enter Edit | Esc Exit",
        super::UiMode::DirBrowser => "↑↓ Navigate | ←→ Expand | Space Toggle | Enter Proceed | Esc Cancel",
        super::UiMode::Dialogue | super::UiMode::DialogueSummary => "↑↓ Navigate | Enter Select | Esc Exit",
        super::UiMode::ClusterDialogue => "↑↓ Select | Enter Keep | Tab Skip | Esc Review",
        super::UiMode::BulkReviewPrompt => "↑↓ Select | Enter Choose | Esc Cancel",
        super::UiMode::SessionReview => "↑↓ Select | Enter Execute | Esc Cancel",
        super::UiMode::DropMissingConfirmation => "↑↓ Scroll | ←→ Select Option | Enter Confirm | Esc Cancel",
        super::UiMode::DeploymentPreview => "↑↓ Navigate | Tab Focus | Enter Confirm | Esc Cancel",
        super::UiMode::CanonClusterView => "↑↓ Navigate | Space Toggle | A All | ←→ Panes | Tab Next | Shift+Tab Back | Esc Review",
        super::UiMode::CanonSessionReview => "↑↓ Scroll | Tab Focus | Enter Commit | Esc Cancel",
        super::UiMode::CanonCommitModal => "↑↓ Select | Enter Confirm | Esc Main Menu",
        super::UiMode::ExitConfirmModal => "←→ Select | Enter/Space Confirm | Y Yes | N/Esc No",
        // Placeholder hints for unimplemented modes
        super::UiMode::CorpusBrowser => "↑↓ Navigate | ←→ Expand | Enter Edit | Esc Exit",
        super::UiMode::AlbumArtistPhaseSelector => "Space Toggle | Enter Proceed | Esc Cancel",
        super::UiMode::AlbumArtistClusterView
        | super::UiMode::AlbumClusterView => "↑↓ Navigate | Space Toggle | Tab Next | Esc Review",
        super::UiMode::AlbumArtistCollation
        | super::UiMode::AlbumArtistPopulation => "↑↓ Navigate | Enter Confirm | Esc Exit",
        super::UiMode::AlbumArtistReview
        | super::UiMode::AlbumArtistCollationReview
        | super::UiMode::AlbumArtistPopulationReview
        | super::UiMode::AlbumReview => "↑↓ Scroll | Enter Commit | Esc Cancel",
        super::UiMode::DirectoryTagEditor => "Tab/Shift+Tab Directories | ↑↓ Fields | Enter Edit | → Action | Esc Exit",
        super::UiMode::DeployConflictReview => "←/→ Select | Enter Confirm | Esc Cancel",
    };
    lines.push(Line::from(hints));

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
