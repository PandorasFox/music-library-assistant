//! Rendering
//!
//! Unified rendering for the tree browser.
//! Dispatches to variant-specific layouts while sharing common tree rendering.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::ui::widgets::control_colors;
use crate::ui::widgets::CURSOR_STYLE;
use crate::ui::widgets::{
    AlbumArtCache, AlbumArtPicker, ArtCacheKey,
    render_album_art_preview, render_no_art_placeholder,
};

use super::entry::{DeployMarker, EntryKind, TreeEntry};
use super::navigator::TreeNavigator;
use super::variants::BrowserVariant;

/// Render the tree browser (titlebar is rendered by render_app).
pub fn render(
    f: &mut Frame,
    area: Rect,
    nav: &mut TreeNavigator,
    variant: &mut BrowserVariant,
    art_picker: &mut AlbumArtPicker,
    art_cache: &mut AlbumArtCache,
) {
    render_corpus_browser(f, area, nav, variant, art_picker, art_cache);
}

/// Render corpus browser layout: content + hint line.
fn render_corpus_browser(
    f: &mut Frame,
    area: Rect,
    nav: &mut TreeNavigator,
    variant: &mut BrowserVariant,
    art_picker: &mut AlbumArtPicker,
    art_cache: &mut AlbumArtCache,
) {
    // Carve out 1 line at the bottom for control hints
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(8),    // Content (tree + optional panel)
            Constraint::Length(1), // Control hints
        ])
        .split(area);

    let content_area = outer[0];
    let hints_area = outer[1];

    // Extract pending edit info from variant for tree rendering
    let BrowserVariant::CorpusBrowser(ref v) = variant;
    let pending_edit_paths = v.pending_edit_paths().clone();
    let corpus_dir = v.corpus_dir().to_path_buf();

    // Determine if we should show art preview:
    // - Config panel NOT open
    // - Selected entry is a file (not directory)
    let show_art = v.config_panel.is_none()
        && nav.current_entry().is_some_and(|e| e.is_file());

    // Check if config panel is open for horizontal split
    if v.config_panel.is_some() {
        // Horizontal split: tree (65%) | config panel (35%)
        let h_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(65), Constraint::Percentage(35)])
            .split(content_area);

        render_corpus_tree(f, h_chunks[0], nav, variant, &pending_edit_paths, &corpus_dir);

        // Render config panel
        let BrowserVariant::CorpusBrowser(ref v) = variant;
        if let Some(ref panel) = v.config_panel {
            panel.render_config_panel(f, h_chunks[1]);
        }
    } else if show_art {
        // Horizontal split: tree (80%) | art preview (20%)
        let h_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(80), Constraint::Percentage(20)])
            .split(content_area);

        render_corpus_tree(f, h_chunks[0], nav, variant, &pending_edit_paths, &corpus_dir);

        // Render art preview for selected file
        let selected_entry = nav.current_entry();
        let selected_path = selected_entry.map(|e| e.path.clone());
        let is_image = selected_entry.is_some_and(|e| e.kind == EntryKind::ImageFile);
        render_file_art_preview(f, h_chunks[1], selected_path.as_deref(), is_image, art_picker, art_cache);
    } else {
        render_corpus_tree(f, content_area, nav, variant, &pending_edit_paths, &corpus_dir);
    }

    // Render control hints
    render_hints(f, hints_area, nav, variant);
}

/// Render a file art preview panel in the corpus browser.
fn render_file_art_preview(
    f: &mut Frame,
    area: Rect,
    file_path: Option<&Path>,
    is_image: bool,
    art_picker: &mut AlbumArtPicker,
    art_cache: &mut AlbumArtCache,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Art")
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.width < 2 || inner.height < 2 {
        return;
    }

    let path = match file_path {
        Some(p) => p,
        None => {
            render_no_art_placeholder(f, inner);
            return;
        }
    };

    // Evict stale cache entries — use appropriate key type
    let key = if is_image {
        ArtCacheKey::Sidecar(path.to_path_buf())
    } else {
        ArtCacheKey::Embedded(path.to_path_buf())
    };
    art_cache.retain_only_keys(&[key]);

    let cached = if is_image {
        art_cache.get_or_load(path, art_picker)
    } else {
        art_cache.get_or_load_embedded(path, art_picker)
    };

    if cached.width == 0 {
        render_no_art_placeholder(f, inner);
    } else {
        // Split into image + metadata
        if inner.height > 3 {
            let meta_height = if !cached.role.is_empty() && cached.role != "other" { 3 } else { 2 };
            let split = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Min(2),
                    Constraint::Length(meta_height),
                ])
                .split(inner);

            render_album_art_preview(f, split[0], cached);

            // Metadata lines: filename, dimensions, and optional role
            let filename = cached.path.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let dim_info = if !cached.role.is_empty() && cached.role != "other" {
                let role_label = if cached.role == "cover_front" { "front" } else { "back" };
                format!("{}x{} {} [{}]", cached.width, cached.height, cached.format.to_uppercase(), role_label)
            } else {
                format!("{}x{} {}", cached.width, cached.height, cached.format.to_uppercase())
            };
            let lines = vec![
                Line::from(Span::styled(filename, Style::default().fg(Color::DarkGray))),
                Line::from(Span::styled(dim_info, Style::default().fg(Color::DarkGray))),
            ];
            f.render_widget(Paragraph::new(lines), split[1]);
        } else {
            render_album_art_preview(f, inner, cached);
        }
    }
}

/// Render the corpus tree area (filter bar + tree pane + overlays).
fn render_corpus_tree(
    f: &mut Frame,
    area: Rect,
    nav: &mut TreeNavigator,
    variant: &mut BrowserVariant,
    pending_edit_paths: &HashSet<PathBuf>,
    corpus_dir: &Path,
) {
    // Layout: Filter bar | Tree (full width)
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Filter bar (3 lines: border + content + border)
            Constraint::Min(5),   // Tree browser
        ])
        .split(area);

    // Filter bar - show filter status or hint
    render_filter_bar(f, main_chunks[0], nav);

    // Tree pane at full width
    render_tree_pane(f, main_chunks[1], nav, pending_edit_paths, corpus_dir);

    // Overlays (match selection modal)
    variant.render_overlays(f, area);
}

/// Render the filter status bar for corpus browser.
fn render_filter_bar(f: &mut Frame, area: Rect, nav: &TreeNavigator) {
    let (content, style) = if nav.has_path_filter() {
        // Active filter - show count and hint to clear
        let count = nav.filtered_file_count().unwrap_or(0);
        (
            format!("Filtered: {} files  (Ctrl+/ to change, Esc to clear)", count),
            Style::default().fg(Color::Green),
        )
    } else {
        // No filter - show hint
        (
            "Press Ctrl+/ to filter files".to_string(),
            Style::default().fg(Color::DarkGray),
        )
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title("Filter");

    let paragraph = Paragraph::new(Span::styled(content, style)).block(block);
    f.render_widget(paragraph, area);
}

/// Render the tree pane.
fn render_tree_pane(
    f: &mut Frame,
    area: Rect,
    nav: &mut TreeNavigator,
    pending_edit_paths: &HashSet<PathBuf>,
    corpus_dir: &Path,
) {
    let inner_height = area.height.saturating_sub(2) as usize;
    nav.set_visible_height(inner_height);

    let entries = nav.entries();
    let cursor_idx = nav.cursor_idx();
    let scroll = nav.scroll_offset();

    let lines: Vec<Line> = entries
        .iter()
        .enumerate()
        .skip(scroll)
        .take(inner_height)
        .map(|(idx, entry)| {
            let is_pending = if entry.is_directory() && !pending_edit_paths.is_empty() {
                // Compute relative path from corpus dir to check against pending edits
                entry.path.strip_prefix(corpus_dir)
                    .ok()
                    .map(|rel| pending_edit_paths.contains(rel))
                    .unwrap_or(false)
            } else {
                false
            };
            render_entry_line(entry, idx == cursor_idx, is_pending)
        })
        .collect();

    let title = format!("Files [{}/{}]", cursor_idx + 1, entries.len());

    let block = Block::default().borders(Borders::ALL).title(title);
    let paragraph = Paragraph::new(lines).block(block);
    f.render_widget(paragraph, area);
}

/// Render context-sensitive control hints at the bottom.
fn render_hints(f: &mut Frame, area: Rect, nav: &TreeNavigator, variant: &BrowserVariant) {
    let BrowserVariant::CorpusBrowser(ref v) = variant;

    let hints = if v.config_panel.is_some() {
        // Config panel is open — hints are shown inside the panel itself
        Line::from(vec![
            control_colors::nav("^v"),
            control_colors::text(" nav  "),
            control_colors::confirm("Enter"),
            control_colors::text(" edit  "),
            control_colors::cancel("Esc"),
            control_colors::text(" close panel"),
        ])
    } else {
        // Standard tree browser hints
        let cursor_entry = nav.current_entry();
        let on_corpus_dir = cursor_entry
            .map(|e| e.is_directory() && e.path.starts_with(v.corpus_dir()))
            .unwrap_or(false);
        let on_dir = cursor_entry.map(|e| e.is_directory()).unwrap_or(false);

        let mut spans = vec![
            control_colors::nav("^v"),
            control_colors::text(" nav  "),
            control_colors::nav("</>"),
            control_colors::text(" expand  "),
            control_colors::confirm("Enter"),
            control_colors::text(if on_dir { " edit dir  " } else { " edit  " }),
            control_colors::toggle("^F"),
            control_colors::text(" filter"),
        ];

        if on_corpus_dir {
            spans.push(control_colors::text("  "));
            spans.push(control_colors::edit("C"));
            spans.push(control_colors::text(" config"));
        }

        if v.has_pending_edits() {
            spans.push(control_colors::text("  "));
            spans.push(control_colors::confirm("R"));
            spans.push(control_colors::text(" review"));
        }

        Line::from(spans)
    };

    f.render_widget(Paragraph::new(hints), area);
}

/// Render a single tree entry line.
fn render_entry_line(entry: &TreeEntry, is_cursor: bool, is_pending_edit: bool) -> Line<'static> {
    let indent = "  ".repeat(entry.depth);

    let expand_indicator = if entry.is_directory() {
        if entry.is_expanded {
            "▽ "
        } else if entry.has_children {
            "▷ "
        } else {
            "  "
        }
    } else {
        "  "
    };

    let icon = match entry.kind {
        EntryKind::Directory => "",
        EntryKind::AudioFile => "♪ ",
        EntryKind::ImageFile => "\u{1f5bc}\u{fe0e} ", // 🖼︎ with text presentation selector
    };

    let count_suffix = if entry.is_directory() {
        match (entry.item_count, entry.image_count) {
            (0, 0) => String::new(),
            (t, 0) => format!("  ({} tracks)", t),
            (0, i) => format!("  ({} images)", i),
            (t, i) => format!("  ({} tracks, {} images)", t, i),
        }
    } else {
        String::new()
    };

    let (deploy_suffix, deploy_style) = match entry.deploy_marker {
        DeployMarker::SourceRoot => ("  [D]", Style::default().fg(Color::Magenta)),
        DeployMarker::Inherited => ("  [d]", Style::default().fg(Color::DarkGray)),
        DeployMarker::None => ("", Style::default()),
    };

    let base_style = if is_cursor {
        CURSOR_STYLE
    } else if entry.is_dimmed {
        Style::default().fg(Color::DarkGray)
    } else if entry.is_directory() {
        Style::default().fg(Color::Blue)
    } else {
        Style::default().fg(Color::White)
    };

    let expand_style = Style::default().fg(Color::Yellow);
    let count_style = Style::default().fg(Color::DarkGray);
    let pending_style = Style::default().fg(Color::Yellow);

    let mut spans = vec![
        Span::raw(indent),
        Span::styled(expand_indicator, expand_style),
        Span::styled(format!("{}{}", icon, entry.name), base_style),
        Span::styled(count_suffix, count_style),
        Span::styled(deploy_suffix, deploy_style),
    ];

    if is_pending_edit {
        spans.push(Span::styled("  [*]", pending_style));
    }

    Line::from(spans)
}
