//! Insights View Rendering
//!
//! Renders the insights view with four buckets:
//! 1. Corpus Files - OOB changes (top), indexed/unindexed/missing counts
//! 2. Placeholder - Reserved (displays `:)`)
//! 3. Library/Deploy - Stale, leftover, ready-to-deploy, deployed healthy
//! 4. Other Signals - Remaining signals sorted by count
//!
//! Dims content when the Witch is busy.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};

use crate::ui::widgets::{LateralView, UnifiedTitleBar};

use super::{FocusedBucket, InsightsViewState};

/// Render the full insights view
pub fn render_insights_view(f: &mut Frame, area: Rect, state: &InsightsViewState) {
    // Layout: Title bar at top, content below
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(UnifiedTitleBar::height()), // Title bar with borders
            Constraint::Min(5),                             // Content
        ])
        .split(area);

    // Render unified title bar
    let titlebar = UnifiedTitleBar::new(LateralView::Insights);
    titlebar.render(f, main_chunks[0]);

    // Layout: Main list on left (65%), details on right (35%)
    let content_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(65), Constraint::Percentage(35)])
        .split(main_chunks[1]);

    render_insights_list(f, content_chunks[0], state);
    render_insight_details(f, content_chunks[1], state);
}

/// Render the insights list with four buckets
fn render_insights_list(f: &mut Frame, area: Rect, state: &InsightsViewState) {
    let busy = state.is_witch_busy();
    let border_color = if busy { Color::DarkGray } else { Color::Gray };

    let block = Block::default()
        .title("Insights")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut items: Vec<ListItem> = Vec::new();

    // Build list items from all buckets
    items.extend(corpus_bucket_items(state, busy));
    items.extend(placeholder_bucket_items(state, busy));
    items.extend(library_bucket_items(state, busy));
    items.extend(other_bucket_items(state, busy));

    let list = List::new(items);
    f.render_widget(list, inner);
}

/// Create bucket header item
fn bucket_header(title: &str, focused: bool, busy: bool) -> ListItem<'static> {
    let style = if busy {
        Style::default().fg(Color::DarkGray)
    } else if focused {
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
    };

    ListItem::new(Line::from(Span::styled(format!("── {} ──", title), style)))
}

/// Create an insight line item
fn insight_line(label: &str, count: usize, selected: bool, busy: bool, color: Color) -> ListItem<'static> {
    let base_color = if busy { Color::DarkGray } else { color };

    let style = if selected && !busy {
        Style::default()
            .fg(Color::Black)
            .bg(base_color)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(base_color)
    };

    let prefix = if selected && !busy { "▶ " } else { "  " };
    let text = format!("{}{}: {}", prefix, label, count);

    ListItem::new(Line::from(Span::styled(text, style)))
}

/// Create an insight line with no count (for placeholder)
fn insight_line_no_count(label: &str, selected: bool, busy: bool, color: Color) -> ListItem<'static> {
    let base_color = if busy { Color::DarkGray } else { color };

    let style = if selected && !busy {
        Style::default()
            .fg(Color::Black)
            .bg(base_color)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(base_color)
    };

    let prefix = if selected && !busy { "▶ " } else { "  " };
    let text = format!("{}{}", prefix, label);

    ListItem::new(Line::from(Span::styled(text, style)))
}

/// Build list items for the Corpus Files bucket
/// OOB signals with count > 0 appear at top; OOB signals with count == 0 sink to bottom
fn corpus_bucket_items(state: &InsightsViewState, busy: bool) -> Vec<ListItem<'static>> {
    let focused = state.focused_bucket == FocusedBucket::Corpus;
    let selected_idx = state.bucket_selections[FocusedBucket::Corpus.index()].selected;

    let data = state.cached_data.as_ref();

    let modified_oob = data.map(|d| d.bucket_corpus.modified_oob).unwrap_or(0);
    let tags_oob = data.map(|d| d.bucket_corpus.tags_changed_oob).unwrap_or(0);
    let files_in_corpus = data.map(|d| d.bucket_corpus.files_in_corpus).unwrap_or(0);
    let indexed = data.map(|d| d.bucket_corpus.files_indexed).unwrap_or(0);
    let unindexed = data.map(|d| d.bucket_corpus.files_unindexed).unwrap_or(0);
    let missing = data.map(|d| d.bucket_corpus.files_missing).unwrap_or(0);
    let relocated = data.map(|d| d.bucket_corpus.files_relocated).unwrap_or(0);

    // Rank: 0 = top (active OOB), 1 = middle (standard), 2 = bottom (inactive OOB)
    struct CorpusEntry {
        label: &'static str,
        count: usize,
        color: Color,
        rank: u8,
    }

    let mut entries = vec![
        CorpusEntry {
            label: "Modified out-of-band",
            count: modified_oob,
            color: if modified_oob > 0 { Color::Red } else { Color::DarkGray },
            rank: if modified_oob > 0 { 0 } else { 2 },
        },
        CorpusEntry {
            label: "Tags changed out-of-band",
            count: tags_oob,
            color: if tags_oob > 0 { Color::Red } else { Color::DarkGray },
            rank: if tags_oob > 0 { 0 } else { 2 },
        },
        CorpusEntry {
            label: "Files in corpus",
            count: files_in_corpus,
            color: Color::Yellow,
            rank: 1,
        },
        CorpusEntry {
            label: "Files indexed",
            count: indexed,
            color: Color::Green,
            rank: 1,
        },
        CorpusEntry {
            label: "Files unindexed",
            count: unindexed,
            color: if unindexed > 0 { Color::Yellow } else { Color::Green },
            rank: 1,
        },
        CorpusEntry {
            label: "Files missing",
            count: missing,
            color: if missing > 0 { Color::Red } else { Color::Green },
            rank: 1,
        },
        CorpusEntry {
            label: "Files relocated",
            count: relocated,
            color: if relocated > 0 { Color::Yellow } else { Color::Green },
            rank: 1,
        },
    ];

    // Sort by rank (0=top, 1=middle, 2=bottom), preserving relative order within ranks
    entries.sort_by_key(|e| e.rank);

    let mut items = vec![bucket_header("Corpus Files", focused, busy)];
    for (idx, entry) in entries.iter().enumerate() {
        items.push(insight_line(
            entry.label,
            entry.count,
            focused && selected_idx == idx,
            busy,
            entry.color,
        ));
    }

    items
}

/// Build list items for the Placeholder bucket
fn placeholder_bucket_items(state: &InsightsViewState, busy: bool) -> Vec<ListItem<'static>> {
    let focused = state.focused_bucket == FocusedBucket::Placeholder;
    let selected_idx = state.bucket_selections[FocusedBucket::Placeholder.index()].selected;

    let data = state.cached_data.as_ref();
    let description = data
        .map(|d| d.bucket_placeholder.description)
        .unwrap_or(":)");

    vec![
        bucket_header("Placeholder", focused, busy),
        insight_line_no_count(
            description,
            focused && selected_idx == 0,
            busy,
            Color::Gray,
        ),
    ]
}

/// Build list items for the Library/Deploy bucket
/// Sorted by count descending (biggest first)
fn library_bucket_items(state: &InsightsViewState, busy: bool) -> Vec<ListItem<'static>> {
    let focused = state.focused_bucket == FocusedBucket::Library;
    let selected_idx = state.bucket_selections[FocusedBucket::Library.index()].selected;

    let data = state.cached_data.as_ref();

    let stale = data.map(|d| d.bucket_library.library_stale).unwrap_or(0);
    let leftover = data.map(|d| d.bucket_library.library_leftover).unwrap_or(0);
    let deploy_ready = data.map(|d| d.bucket_library.deploy_ready).unwrap_or(0);
    let deployed = data.map(|d| d.bucket_library.deployed_healthy).unwrap_or(0);

    // Build entries with their color logic, then sort by count descending
    struct DeployEntry {
        label: &'static str,
        count: usize,
        color: Color,
    }

    let mut entries = vec![
        DeployEntry {
            label: "Library stale",
            count: stale,
            color: if stale > 0 { Color::Yellow } else { Color::Green },
        },
        DeployEntry {
            label: "Library leftover",
            count: leftover,
            color: if leftover > 0 { Color::Yellow } else { Color::Green },
        },
        DeployEntry {
            label: "Ready to deploy",
            count: deploy_ready,
            color: if deploy_ready > 0 { Color::Cyan } else { Color::Green },
        },
        DeployEntry {
            label: "Deployed healthy",
            count: deployed,
            color: Color::Green,
        },
    ];

    // Sort by count descending (biggest first)
    entries.sort_by(|a, b| b.count.cmp(&a.count));

    let mut items = vec![bucket_header("Library / Deploy", focused, busy)];
    for (idx, entry) in entries.iter().enumerate() {
        items.push(insight_line(
            entry.label,
            entry.count,
            focused && selected_idx == idx,
            busy,
            entry.color,
        ));
    }

    items
}

/// Build list items for the Other Signals bucket
fn other_bucket_items(state: &InsightsViewState, busy: bool) -> Vec<ListItem<'static>> {
    let focused = state.focused_bucket == FocusedBucket::Other;
    let selected_idx = state.bucket_selections[FocusedBucket::Other.index()].selected;

    let mut items = vec![bucket_header("Other Signals", focused, busy)];

    if let Some(data) = state.cached_data.as_ref() {
        for (idx, entry) in data.bucket_other.entries.iter().enumerate() {
            let color = if entry.count > 0 { Color::Yellow } else { Color::Green };
            items.push(insight_line(
                &entry.display_label,
                entry.count,
                focused && selected_idx == idx,
                busy,
                color,
            ));
        }
    }

    // If no entries, show a placeholder
    if items.len() == 1 {
        items.push(insight_line_no_count(
            "(no other signals)",
            focused && selected_idx == 0,
            busy,
            Color::DarkGray,
        ));
    }

    items
}

/// Render the details pane for the currently selected insight
fn render_insight_details(f: &mut Frame, area: Rect, state: &InsightsViewState) {
    let busy = state.is_witch_busy();
    let border_color = if busy { Color::DarkGray } else { Color::Gray };

    let block = Block::default()
        .title("Details")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let lines = match state.focused_bucket {
        FocusedBucket::Corpus => corpus_detail_lines(state, busy),
        FocusedBucket::Placeholder => placeholder_detail_lines(busy),
        FocusedBucket::Library => library_detail_lines(state, busy),
        FocusedBucket::Other => other_detail_lines(state, busy),
    };

    let paragraph = Paragraph::new(lines);
    f.render_widget(paragraph, inner);
}

/// Detail lines for corpus bucket items
fn corpus_detail_lines(state: &InsightsViewState, busy: bool) -> Vec<Line<'static>> {
    let selected_idx = state.bucket_selections[FocusedBucket::Corpus.index()].selected;
    let text_color = if busy { Color::DarkGray } else { Color::White };
    let header_color = if busy { Color::DarkGray } else { Color::Cyan };

    let mut lines = Vec::new();

    match selected_idx {
        0 => {
            // Modified out-of-band
            lines.push(Line::from(Span::styled(
                "Modified Out-of-Band",
                Style::default().fg(header_color).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Files modified outside MLA since",
                Style::default().fg(text_color),
            )));
            lines.push(Line::from(Span::styled(
                "indexing. May need re-indexing or",
                Style::default().fg(text_color),
            )));
            lines.push(Line::from(Span::styled(
                "investigation.",
                Style::default().fg(text_color),
            )));
        }
        1 => {
            // Tags changed out-of-band
            lines.push(Line::from(Span::styled(
                "Tags Changed Out-of-Band",
                Style::default().fg(header_color).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "File tags were modified externally.",
                Style::default().fg(text_color),
            )));
            lines.push(Line::from(Span::styled(
                "Database metadata may be stale.",
                Style::default().fg(text_color),
            )));
        }
        2 => {
            // Files in corpus - show filetype breakdown
            lines.push(Line::from(Span::styled(
                "Files in Corpus",
                Style::default().fg(header_color).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));

            if let Some(data) = state.cached_data.as_ref() {
                if !data.bucket_corpus.file_type_breakdown.is_empty() {
                    lines.push(Line::from(Span::styled(
                        "By file type:",
                        Style::default().fg(text_color),
                    )));
                    for (ext, count) in &data.bucket_corpus.file_type_breakdown {
                        lines.push(Line::from(Span::styled(
                            format!("  .{}: {}", ext, count),
                            Style::default().fg(text_color),
                        )));
                    }
                } else {
                    lines.push(Line::from(Span::styled(
                        "No files found.",
                        Style::default().fg(text_color),
                    )));
                }
            } else {
                lines.push(Line::from(Span::styled(
                    "Loading...",
                    Style::default().fg(Color::DarkGray),
                )));
            }
        }
        3 => {
            // Files indexed
            lines.push(Line::from(Span::styled(
                "Files Indexed",
                Style::default().fg(header_color).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Files with complete metadata",
                Style::default().fg(text_color),
            )));
            lines.push(Line::from(Span::styled(
                "in the database.",
                Style::default().fg(text_color),
            )));
        }
        4 => {
            // Files unindexed
            lines.push(Line::from(Span::styled(
                "Files Unindexed",
                Style::default().fg(header_color).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Audio files in corpus not yet",
                Style::default().fg(text_color),
            )));
            lines.push(Line::from(Span::styled(
                "indexed. Run indexing to process.",
                Style::default().fg(text_color),
            )));
        }
        5 => {
            // Files missing
            lines.push(Line::from(Span::styled(
                "Files Missing",
                Style::default().fg(header_color).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Indexed files no longer found",
                Style::default().fg(text_color),
            )));
            lines.push(Line::from(Span::styled(
                "at expected path. May have been",
                Style::default().fg(text_color),
            )));
            lines.push(Line::from(Span::styled(
                "moved or deleted.",
                Style::default().fg(text_color),
            )));
        }
        6 => {
            // Files relocated
            lines.push(Line::from(Span::styled(
                "Files Relocated",
                Style::default().fg(header_color).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Files moved within corpus.",
                Style::default().fg(text_color),
            )));
            lines.push(Line::from(Span::styled(
                "Database paths need updating.",
                Style::default().fg(text_color),
            )));
        }
        _ => {}
    }

    lines
}

/// Detail lines for placeholder bucket
fn placeholder_detail_lines(busy: bool) -> Vec<Line<'static>> {
    let text_color = if busy { Color::DarkGray } else { Color::Gray };

    vec![
        Line::from(""),
        Line::from(Span::styled(
            "Reserved for future use.",
            Style::default().fg(text_color),
        )),
    ]
}

/// Detail lines for library bucket items
fn library_detail_lines(state: &InsightsViewState, busy: bool) -> Vec<Line<'static>> {
    let selected_idx = state.bucket_selections[FocusedBucket::Library.index()].selected;
    let text_color = if busy { Color::DarkGray } else { Color::White };
    let header_color = if busy { Color::DarkGray } else { Color::Cyan };

    let mut lines = Vec::new();

    match selected_idx {
        0 => {
            // Library stale
            lines.push(Line::from(Span::styled(
                "Library Stale",
                Style::default().fg(header_color).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Library links pointing to outdated",
                Style::default().fg(text_color),
            )));
            lines.push(Line::from(Span::styled(
                "corpus paths. Re-deploy to fix.",
                Style::default().fg(text_color),
            )));
        }
        1 => {
            // Library leftover
            lines.push(Line::from(Span::styled(
                "Library Leftover",
                Style::default().fg(header_color).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Files in library not linked to",
                Style::default().fg(text_color),
            )));
            lines.push(Line::from(Span::styled(
                "any corpus file. Orphaned links.",
                Style::default().fg(text_color),
            )));
        }
        2 => {
            // Ready to deploy
            lines.push(Line::from(Span::styled(
                "Ready to Deploy",
                Style::default().fg(header_color).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Healthy corpus files not yet",
                Style::default().fg(text_color),
            )));
            lines.push(Line::from(Span::styled(
                "present in any library.",
                Style::default().fg(text_color),
            )));
        }
        3 => {
            // Deployed healthy
            lines.push(Line::from(Span::styled(
                "Deployed Healthy",
                Style::default().fg(header_color).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Corpus files successfully",
                Style::default().fg(text_color),
            )));
            lines.push(Line::from(Span::styled(
                "deployed to library at correct",
                Style::default().fg(text_color),
            )));
            lines.push(Line::from(Span::styled(
                "paths.",
                Style::default().fg(text_color),
            )));
        }
        _ => {}
    }

    lines
}

/// Detail lines for other signals bucket items
fn other_detail_lines(state: &InsightsViewState, busy: bool) -> Vec<Line<'static>> {
    let selected_idx = state.bucket_selections[FocusedBucket::Other.index()].selected;
    let text_color = if busy { Color::DarkGray } else { Color::White };
    let header_color = if busy { Color::DarkGray } else { Color::Cyan };

    let mut lines = Vec::new();

    if let Some(data) = state.cached_data.as_ref() {
        if let Some(entry) = data.bucket_other.entries.get(selected_idx) {
            lines.push(Line::from(Span::styled(
                entry.display_label.clone(),
                Style::default().fg(header_color).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!("Count: {}", entry.count),
                Style::default().fg(text_color),
            )));
            if let Some(affected) = entry.affected_count {
                lines.push(Line::from(Span::styled(
                    format!("Affected tracks: {}", affected),
                    Style::default().fg(text_color),
                )));
            }
        }
    }

    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "No signal selected.",
            Style::default().fg(Color::DarkGray),
        )));
    }

    lines
}
