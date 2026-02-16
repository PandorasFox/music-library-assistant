//! Insights View Rendering
//!
//! Renders the insights view with three buckets:
//! 1. Corpus Files - OOB changes (top), indexed/unindexed/missing counts
//! 2. Tag & Duplicate Issues - Canonicity, compound splits, duplicates
//! 3. Other Signals - Remaining signals sorted by count
//!
//! Dims content when the Witch is busy.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};

use crate::ui::helpers::render_pane;

use super::{BucketEntry, FocusedBucket, InsightType, InsightsViewState};

/// Render the full insights view (titlebar is rendered by render_app).
pub fn render_insights_view(f: &mut Frame, area: Rect, state: &mut InsightsViewState) {
    // Layout: Main list on left (65%), details on right (35%)
    let content_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(65), Constraint::Percentage(35)])
        .split(area);

    render_insights_list(f, content_chunks[0], state);
    render_insight_details(f, content_chunks[1], &*state);
}

/// Render the insights list with four buckets
fn render_insights_list(f: &mut Frame, area: Rect, state: &mut InsightsViewState) {
    let busy = state.is_witch_busy();
    let border_color = if busy { Color::DarkGray } else { Color::Gray };

    let block = Block::default()
        .title("Insights")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let inner = render_pane(f, area, block);

    // Clear and set up click targets
    state.click_targets.clear();
    state.click_targets.set_list_area(inner);

    let mut items: Vec<ListItem> = Vec::new();
    let mut y = inner.y;

    // Build list items from all buckets using cached entries, tracking Y positions
    let (corpus_items, corpus_y) = bucket_items_with_targets(
        "Corpus Files",
        &state.cached_entries.corpus,
        FocusedBucket::Corpus,
        &state.focused_bucket,
        &state.bucket_selections,
        busy,
        y,
        &mut state.click_targets,
    );
    items.extend(corpus_items);
    y = corpus_y;

    let (placeholder_items, placeholder_y) = bucket_items_with_targets(
        "Similar Tag Issues",
        &state.cached_entries.placeholder,
        FocusedBucket::Placeholder,
        &state.focused_bucket,
        &state.bucket_selections,
        busy,
        y,
        &mut state.click_targets,
    );
    items.extend(placeholder_items);
    y = placeholder_y;

    let (other_items, _) = bucket_items_other_with_targets(
        "Other Signals",
        &state.cached_entries.other,
        &state.focused_bucket,
        &state.bucket_selections,
        busy,
        y,
        &mut state.click_targets,
    );
    items.extend(other_items);

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

use super::{BucketSelection, InsightsClickTargets};

/// Build list items from pre-sorted bucket entries with click target tracking.
/// Returns the items and the next Y position.
#[allow(clippy::too_many_arguments)]
fn bucket_items_with_targets(
    title: &str,
    entries: &[BucketEntry],
    bucket: FocusedBucket,
    focused_bucket: &FocusedBucket,
    bucket_selections: &[BucketSelection; 3],
    busy: bool,
    start_y: u16,
    click_targets: &mut InsightsClickTargets,
) -> (Vec<ListItem<'static>>, u16) {
    let focused = *focused_bucket == bucket;
    let selected_idx = bucket_selections[bucket.index()].selected;

    let mut items = vec![bucket_header(title, focused, busy)];
    let mut y = start_y;

    // Header row (not clickable)
    click_targets.add_header(bucket, y);
    y += 1;

    for (idx, entry) in entries.iter().enumerate() {
        let selected = focused && selected_idx == idx;
        match entry.count {
            Some(count) => items.push(insight_line(&entry.label, count, selected, busy, entry.color)),
            None => items.push(insight_line_no_count(&entry.label, selected, busy, entry.color)),
        }
        // Add click target for this item
        click_targets.add_item(bucket, idx, y);
        y += 1;
    }

    (items, y)
}

/// Build list items for the Other bucket with click target tracking.
/// Returns the items and the next Y position.
fn bucket_items_other_with_targets(
    title: &str,
    entries: &[BucketEntry],
    focused_bucket: &FocusedBucket,
    bucket_selections: &[BucketSelection; 3],
    busy: bool,
    start_y: u16,
    click_targets: &mut InsightsClickTargets,
) -> (Vec<ListItem<'static>>, u16) {
    let focused = *focused_bucket == FocusedBucket::Other;
    let selected_idx = bucket_selections[FocusedBucket::Other.index()].selected;

    let mut items = vec![bucket_header(title, focused, busy)];
    let mut y = start_y;

    // Header row (not clickable)
    click_targets.add_header(FocusedBucket::Other, y);
    y += 1;

    if entries.is_empty() {
        items.push(insight_line_no_count(
            "(no other signals)",
            focused && selected_idx == 0,
            busy,
            Color::DarkGray,
        ));
        // Empty placeholder row - no click target needed
        y += 1;
    } else {
        for (idx, entry) in entries.iter().enumerate() {
            let selected = focused && selected_idx == idx;
            match entry.count {
                Some(count) => items.push(insight_line(&entry.label, count, selected, busy, entry.color)),
                None => items.push(insight_line_no_count(&entry.label, selected, busy, entry.color)),
            }
            // Add click target for this item
            click_targets.add_item(FocusedBucket::Other, idx, y);
            y += 1;
        }
    }

    (items, y)
}

/// Render the details pane for the currently selected insight
fn render_insight_details(f: &mut Frame, area: Rect, state: &InsightsViewState) {
    let busy = state.is_witch_busy();
    let border_color = if busy { Color::DarkGray } else { Color::Gray };

    let block = Block::default()
        .title("Details")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let inner = render_pane(f, area, block);

    let lines = match state.selected_entry() {
        Some(entry) => detail_lines_for_entry(entry, state, busy),
        None => vec![Line::from(Span::styled(
            "No insight selected.",
            Style::default().fg(Color::DarkGray),
        ))],
    };

    let paragraph = Paragraph::new(lines);
    f.render_widget(paragraph, inner);
}

/// Generate detail lines for a bucket entry, matching on InsightType
fn detail_lines_for_entry(entry: &BucketEntry, state: &InsightsViewState, busy: bool) -> Vec<Line<'static>> {
    let text_color = if busy { Color::DarkGray } else { Color::White };
    let header_color = if busy { Color::DarkGray } else { Color::Cyan };

    let mut lines = Vec::new();

    match entry.insight_type {
        // Corpus bucket entries
        InsightType::CorpusMtimeOnly => {
            lines.push(Line::from(Span::styled(
                "Mtime-Only Changes",
                Style::default().fg(header_color).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Files touched but tags unchanged.",
                Style::default().fg(text_color),
            )));
            lines.push(Line::from(Span::styled(
                "Acknowledge to update scan state",
                Style::default().fg(text_color),
            )));
            lines.push(Line::from(Span::styled(
                "without modifying files.",
                Style::default().fg(text_color),
            )));
        }
        InsightType::CorpusOobTagSync => {
            lines.push(Line::from(Span::styled(
                "Tags Syncable (Out-of-Band)",
                Style::default().fg(header_color).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Files have extra tags in one direction",
                Style::default().fg(text_color),
            )));
            lines.push(Line::from(Span::styled(
                "only: either on disk or in the index.",
                Style::default().fg(text_color),
            )));
            lines.push(Line::from(Span::styled(
                "Can be synced to bring both in line.",
                Style::default().fg(text_color),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Press Enter to resolve.",
                Style::default().fg(Color::Cyan),
            )));
        }
        InsightType::CorpusOobTagConflict => {
            lines.push(Line::from(Span::styled(
                "Tag Conflicts (Out-of-Band)",
                Style::default().fg(header_color).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Files have tag values that differ",
                Style::default().fg(text_color),
            )));
            lines.push(Line::from(Span::styled(
                "between disk and database, or have",
                Style::default().fg(text_color),
            )));
            lines.push(Line::from(Span::styled(
                "extras in both directions.",
                Style::default().fg(text_color),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Press Enter to inspect.",
                Style::default().fg(Color::Cyan),
            )));
        }
        InsightType::CorpusCorruptFiles => {
            lines.push(Line::from(Span::styled(
                "Corrupt Files",
                Style::default().fg(header_color).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Files that failed to read during",
                Style::default().fg(text_color),
            )));
            lines.push(Line::from(Span::styled(
                "tag verification or waveform decoding.",
                Style::default().fg(text_color),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Press Enter to stash and drop.",
                Style::default().fg(Color::Cyan),
            )));
        }
        InsightType::CorpusShitFormatFiles => {
            lines.push(Line::from(Span::styled(
                "Shit Format Files",
                Style::default().fg(header_color).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Non-Vorbis container files (MP3, M4A,",
                Style::default().fg(text_color),
            )));
            lines.push(Line::from(Span::styled(
                "WAV, etc.) with poor metadata support.",
                Style::default().fg(text_color),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Press Enter to transcode to Opus.",
                Style::default().fg(Color::Cyan),
            )));
        }
        InsightType::CorpusFilesInCorpus => {
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
        InsightType::CorpusFilesIndexed => {
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
        InsightType::CorpusFilesUnindexed => {
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
        InsightType::CorpusFilesMissing => {
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
        InsightType::CorpusDirectoriesMissing => {
            lines.push(Line::from(Span::styled(
                "Directories Missing",
                Style::default().fg(header_color).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Indexed directories no longer found",
                Style::default().fg(text_color),
            )));
            lines.push(Line::from(Span::styled(
                "on disk. May have been moved or",
                Style::default().fg(text_color),
            )));
            lines.push(Line::from(Span::styled(
                "deleted externally.",
                Style::default().fg(text_color),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Press Enter to drop from index.",
                Style::default().fg(Color::Cyan),
            )));
        }
        InsightType::CorpusFilesRelocated => {
            lines.push(Line::from(Span::styled(
                "Files Relocated (Moved)",
                Style::default().fg(header_color).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Files moved within corpus (same inode,",
                Style::default().fg(text_color),
            )));
            lines.push(Line::from(Span::styled(
                "different path). Database paths need",
                Style::default().fg(text_color),
            )));
            lines.push(Line::from(Span::styled(
                "updating to match new locations.",
                Style::default().fg(text_color),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Press Enter to acknowledge and update paths.",
                Style::default().fg(Color::Cyan),
            )));
        }

        // Tag resolution bucket entries (duplicates at top)
        InsightType::CrossSourceOverlaps => {
            lines.push(Line::from(Span::styled(
                "Cross-Source Overlaps",
                Style::default().fg(header_color).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Same tracks exist in different source",
                Style::default().fg(text_color),
            )));
            lines.push(Line::from(Span::styled(
                "directories (e.g., bandcamp vs indie).",
                Style::default().fg(text_color),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Press Enter to resolve by source.",
                Style::default().fg(if busy { Color::DarkGray } else { Color::Cyan }),
            )));
        }

        InsightType::SubparDuplicates => {
            lines.push(Line::from(Span::styled(
                "Subpar Duplicates",
                Style::default().fg(header_color).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Lower quality versions of tracks",
                Style::default().fg(text_color),
            )));
            lines.push(Line::from(Span::styled(
                "identified by fingerprint analysis.",
                Style::default().fg(text_color),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Press Enter to stash subpar copies.",
                Style::default().fg(if busy { Color::DarkGray } else { Color::Cyan }),
            )));
        }

        InsightType::RedundantDuplicates => {
            lines.push(Line::from(Span::styled(
                "Redundant Duplicates",
                Style::default().fg(header_color).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Same fingerprint, identical quality.",
                Style::default().fg(text_color),
            )));
            lines.push(Line::from(Span::styled(
                "Neither file is subpar — requires",
                Style::default().fg(text_color),
            )));
            lines.push(Line::from(Span::styled(
                "operator choice.",
                Style::default().fg(text_color),
            )));
        }

        InsightType::InconsistentAlbumArtist => {
            lines.push(Line::from(Span::styled(
                "Inconsistent Album Artist",
                Style::default().fg(header_color).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Albums with multiple artists but",
                Style::default().fg(text_color),
            )));
            lines.push(Line::from(Span::styled(
                "missing or inconsistent album_artist.",
                Style::default().fg(text_color),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Press Enter to resolve.",
                Style::default().fg(if busy { Color::DarkGray } else { Color::Cyan }),
            )));
        }

        InsightType::TagCanonicity { ref tag_name } => {
            lines.push(Line::from(Span::styled(
                format!("{} Canonicity", tag_name),
                Style::default().fg(header_color).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!("Variants of {} tags that should", tag_name),
                Style::default().fg(text_color),
            )));
            lines.push(Line::from(Span::styled(
                "be unified (e.g., spelling differences).",
                Style::default().fg(text_color),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Press Enter to resolve.",
                Style::default().fg(if busy { Color::DarkGray } else { Color::Cyan }),
            )));
        }

        InsightType::CompoundTagValueSafe { ref tag_name } => {
            lines.push(Line::from(Span::styled(
                format!("{} Compound Splits (Safe)", tag_name),
                Style::default().fg(header_color).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!("All split parts for {} tags already", tag_name),
                Style::default().fg(text_color),
            )));
            lines.push(Line::from(Span::styled(
                "exist in corpus. Safe to split in bulk.",
                Style::default().fg(text_color),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Press Enter to bulk split.",
                Style::default().fg(if busy { Color::DarkGray } else { Color::Green }),
            )));
        }

        InsightType::CompoundTagValueReview { ref tag_name } => {
            lines.push(Line::from(Span::styled(
                format!("{} Compound Splits (Review)", tag_name),
                Style::default().fg(header_color).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!("Some split parts for {} tags are", tag_name),
                Style::default().fg(text_color),
            )));
            lines.push(Line::from(Span::styled(
                "new to corpus. Review each to verify.",
                Style::default().fg(text_color),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Press Enter to review.",
                Style::default().fg(if busy { Color::DarkGray } else { Color::Yellow }),
            )));
        }

        InsightType::EmbeddableAlbumArt => {
            lines.push(Line::from(Span::styled(
                "Embeddable Album Art",
                Style::default().fg(header_color).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Directories with sidecar images",
                Style::default().fg(text_color),
            )));
            lines.push(Line::from(Span::styled(
                "(cover.jpg, folder.png, etc.)",
                Style::default().fg(text_color),
            )));
            lines.push(Line::from(Span::styled(
                "alongside audio files without",
                Style::default().fg(text_color),
            )));
            lines.push(Line::from(Span::styled(
                "embedded pictures.",
                Style::default().fg(text_color),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Press Enter to embed.",
                Style::default().fg(if busy { Color::DarkGray } else { Color::Cyan }),
            )));
        }

        InsightType::MissingAlbumSingle => {
            lines.push(Line::from(Span::styled(
                "Missing Album Singles",
                Style::default().fg(header_color).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Tracks with ARTIST and TITLE but no",
                Style::default().fg(text_color),
            )));
            lines.push(Line::from(Span::styled(
                "ALBUM tag, grouped by artist.",
                Style::default().fg(text_color),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Press Enter to assign album values.",
                Style::default().fg(if busy { Color::DarkGray } else { Color::Cyan }),
            )));
        }

        // Other bucket - dynamic entries
        InsightType::OtherSignal { index } => {
            // Get extended info from cached_data if available
            if let Some(data) = state.cached_data.as_ref() {
                if let Some(signal_entry) = data.bucket_other.entries.get(index) {
                    lines.push(Line::from(Span::styled(
                        signal_entry.display_label.clone(),
                        Style::default().fg(header_color).add_modifier(Modifier::BOLD),
                    )));
                    lines.push(Line::from(""));
                    lines.push(Line::from(Span::styled(
                        format!("Count: {}", signal_entry.count),
                        Style::default().fg(text_color),
                    )));
                    if let Some(affected) = signal_entry.affected_count {
                        lines.push(Line::from(Span::styled(
                            format!("Affected tracks: {}", affected),
                            Style::default().fg(text_color),
                        )));
                    }
                }
            }

            // Fallback if data not available
            if lines.is_empty() {
                lines.push(Line::from(Span::styled(
                    entry.label.clone(),
                    Style::default().fg(header_color).add_modifier(Modifier::BOLD),
                )));
                lines.push(Line::from(""));
                if let Some(count) = entry.count {
                    lines.push(Line::from(Span::styled(
                        format!("Count: {}", count),
                        Style::default().fg(text_color),
                    )));
                }
            }
        }
    }

    lines
}
