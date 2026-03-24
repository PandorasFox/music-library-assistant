//! Rendering
//!
//! Unified rendering for the tree browser.
//! Uses BrowserEntry (mm-ui) with variant-provided marker lookups for
//! deploy/packing/dimming decorations.

use std::collections::HashSet;
use std::path::PathBuf;

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use mm_ui::directory_browser::{BrowserEntry, DirectoryBrowser};

use super::entry::{DeployMarker, PackingMarker};
use crate::packing_colors::PackingCategoryColor;
use mm_meta::paths::PathResolver;
use crate::widgets::control_colors;
use crate::widgets::CURSOR_STYLE;
use crate::widgets::{
    render_album_art_preview, render_no_art_placeholder, AlbumArtCache, AlbumArtPicker, ArtCacheKey,
};

use crate::widgets::ListClickTargets;

use super::variants::BrowserVariant;
use crate::widgets::wizard::WizardOffer;
use crate::widgets::wizard_pane::render_wizard_pane;
use crate::widgets::wizard_popup::WizardPopup;

/// Render the tree browser (titlebar is rendered by render_app).
pub fn render(
    f: &mut Frame,
    area: Rect,
    browser: &mut DirectoryBrowser,
    variant: &mut BrowserVariant,
    art_picker: &mut AlbumArtPicker,
    art_cache: &mut AlbumArtCache,
    click_targets: &mut ListClickTargets,
    resolver: &PathResolver,
) {
    render_corpus_browser(f, area, browser, variant, art_picker, art_cache, click_targets, resolver);
}

/// Render corpus browser layout: content + hint line.
fn render_corpus_browser(
    f: &mut Frame,
    area: Rect,
    browser: &mut DirectoryBrowser,
    variant: &mut BrowserVariant,
    art_picker: &mut AlbumArtPicker,
    art_cache: &mut AlbumArtCache,
    click_targets: &mut ListClickTargets,
    resolver: &PathResolver,
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

    // Extract variant state for rendering
    let BrowserVariant::CorpusBrowser(ref v) = variant;
    let pending_edit_paths = v.pending_edit_paths().clone();
    let wizard_state = v.wizard_state();
    let wizard_offer_snapshot = v.wizard_offer().cloned();

    // Determine if we should show art preview:
    // - Dir config panel NOT open
    // - Wizard pane NOT showing
    // - Selected entry is a file (not directory)
    let show_art = v.dir_config_panel.is_none()
        && !wizard_state.is_showing_pane()
        && browser.current_entry().is_some_and(|e| !e.is_dir);

    let mut tree_area = content_area;

    if v.dir_config_panel.is_some() {
        // Horizontal split: tree (65%) | config panel (35%)
        let h_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(65), Constraint::Percentage(35)])
            .split(content_area);

        tree_area = h_chunks[0];
        render_corpus_tree(f, h_chunks[0], browser, variant, &pending_edit_paths, click_targets);

        let BrowserVariant::CorpusBrowser(ref v) = variant;
        if let Some(ref panel) = v.dir_config_panel {
            panel.render(f, h_chunks[1]);
        }
    } else if wizard_state.is_showing_pane() {
        // Horizontal split: tree (60%) | wizard pane (40%)
        let h_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(content_area);

        tree_area = h_chunks[0];
        render_corpus_tree(f, h_chunks[0], browser, variant, &pending_edit_paths, click_targets);

        let BrowserVariant::CorpusBrowser(ref mut v) = variant;
        if let Some(ref offer) = wizard_offer_snapshot {
            let (title, content) = match offer {
                WizardOffer::Pane { title, content } => (title.as_str(), content.as_slice()),
                WizardOffer::Both {
                    pane_title,
                    pane_content,
                    ..
                } => (pane_title.as_str(), pane_content.as_slice()),
                WizardOffer::Popup(_) => ("Info", &[][..]),
            };
            render_wizard_pane(f, h_chunks[1], title, content, v.wizard_pane_mut(), false);
        }
    } else if show_art {
        // Horizontal split: tree (80%) | art preview (20%)
        let h_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(80), Constraint::Percentage(20)])
            .split(content_area);

        tree_area = h_chunks[0];
        render_corpus_tree(f, h_chunks[0], browser, variant, &pending_edit_paths, click_targets);

        // Render art preview: resolve relative path to absolute for file loading
        let selected_entry = browser.current_entry();
        let selected_abs_path = selected_entry
            .map(|e| resolver.resolve(std::path::Path::new(&e.path)));
        render_file_art_preview(
            f,
            h_chunks[1],
            selected_abs_path.as_deref(),
            false, // BrowserEntry doesn't distinguish image files; treat as audio
            art_picker,
            art_cache,
        );
    } else {
        render_corpus_tree(f, content_area, browser, variant, &pending_edit_paths, click_targets);
    }

    // Overlay wizard popup when showing
    if wizard_state.is_showing_popup() {
        if let Some(ref offer) = wizard_offer_snapshot {
            let popup_lines = match offer {
                WizardOffer::Popup(lines) => lines.as_slice(),
                WizardOffer::Both { popup, .. } => popup.as_slice(),
                WizardOffer::Pane { .. } => &[],
            };
            if !popup_lines.is_empty() {
                let cursor_idx = browser.cursor;
                let scroll = browser.scroll;
                let anchor_y = tree_area.y + 1 + (cursor_idx.saturating_sub(scroll)) as u16;
                let cursor_entry = browser.current_entry();
                let entry_width = cursor_entry
                    .map(|e| {
                        let indent = e.depth * 2;
                        let expand = 2;
                        let name_len = e.name.chars().count();
                        indent + expand + name_len + 10
                    })
                    .unwrap_or(20);
                let anchor_x = tree_area.x + 1 + entry_width as u16;
                WizardPopup::render(f, popup_lines, anchor_x, anchor_y, tree_area);
            }
        }
    }

    render_hints(f, hints_area, browser, variant);
}

/// Render a file art preview panel in the corpus browser.
fn render_file_art_preview(
    f: &mut Frame,
    area: Rect,
    file_path: Option<&std::path::Path>,
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
        if inner.height > 3 {
            let meta_height = if !cached.role.is_empty() && cached.role != "other" {
                3
            } else {
                2
            };
            let split = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(2), Constraint::Length(meta_height)])
                .split(inner);

            render_album_art_preview(f, split[0], cached);

            let filename = cached
                .path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let dim_info = if !cached.role.is_empty() && cached.role != "other" {
                let role_label = if cached.role == "cover_front" {
                    "front"
                } else {
                    "back"
                };
                format!(
                    "{}x{} {} [{}]",
                    cached.width, cached.height, cached.format.to_uppercase(), role_label
                )
            } else {
                format!(
                    "{}x{} {}",
                    cached.width, cached.height, cached.format.to_uppercase()
                )
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
    browser: &mut DirectoryBrowser,
    variant: &mut BrowserVariant,
    pending_edit_paths: &HashSet<PathBuf>,
    click_targets: &mut ListClickTargets,
) {
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Filter bar
            Constraint::Min(5),    // Tree browser
        ])
        .split(area);

    render_filter_bar(f, main_chunks[0], browser, variant);

    render_tree_pane(f, main_chunks[1], browser, variant, pending_edit_paths, click_targets);

    variant.render_overlays(f, area);
}

/// Render the filter status bar for corpus browser.
fn render_filter_bar(f: &mut Frame, area: Rect, browser: &DirectoryBrowser, variant: &BrowserVariant) {
    let BrowserVariant::CorpusBrowser(ref v) = variant;

    if v.filter_active() {
        let input = v.filter_input();
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow))
            .title("Filter");

        let inner = block.inner(area);
        f.render_widget(block, area);

        let label = Span::styled("Filter: ", Style::default().fg(Color::Yellow));
        let text = Span::styled(input.value(), Style::default().fg(Color::White));
        let line = Line::from(vec![label, text]);
        f.render_widget(Paragraph::new(line), inner);

        let cursor_x = inner.x + 8 + input.cursor as u16;
        if cursor_x < inner.x + inner.width {
            f.set_cursor_position((cursor_x, inner.y));
        }
    } else if browser.has_path_filter() {
        let count = browser.filtered_file_count().unwrap_or(0);
        let content = format!("Filtered: {} files  (Esc to clear)", count);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Green))
            .title("Filter");
        let paragraph =
            Paragraph::new(Span::styled(content, Style::default().fg(Color::Green))).block(block);
        f.render_widget(paragraph, area);
    } else {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title("Filter");
        let paragraph = Paragraph::new(Span::styled(
            "Ctrl+/ to filter",
            Style::default().fg(Color::DarkGray),
        ))
        .block(block);
        f.render_widget(paragraph, area);
    }
}

/// Render the tree pane.
fn render_tree_pane(
    f: &mut Frame,
    area: Rect,
    browser: &mut DirectoryBrowser,
    variant: &BrowserVariant,
    pending_edit_paths: &HashSet<PathBuf>,
    click_targets: &mut ListClickTargets,
) {
    let inner_height = area.height.saturating_sub(2) as usize;
    browser.visible_height = inner_height;

    let entries = &browser.entries;
    let cursor_idx = browser.cursor;
    let scroll = browser.scroll;

    let inner_area = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };
    click_targets.populate(inner_area, scroll, entries.len());

    let BrowserVariant::CorpusBrowser(ref v) = variant;

    let lines: Vec<Line> = entries
        .iter()
        .enumerate()
        .skip(scroll)
        .take(inner_height)
        .map(|(idx, entry)| {
            // Compute markers from variant lookup data
            let deploy_marker = v.deploy_marker_for(&entry.path);
            let packing_marker = v.packing_marker_for(&entry.path, entry.is_dir);
            let is_dimmed = entry.depth == 0 && v.is_dimmed(&entry.path);

            let is_pending = if entry.is_dir && !pending_edit_paths.is_empty() {
                // Entry paths are already zone-relative (config-relative).
                pending_edit_paths.contains(std::path::Path::new(&entry.path))
            } else {
                false
            };

            render_entry_line(entry, idx == cursor_idx, is_pending, deploy_marker, packing_marker, is_dimmed)
        })
        .collect();

    let title = format!("Files [{}/{}]", cursor_idx + 1, entries.len());
    let block = Block::default().borders(Borders::ALL).title(title);
    let paragraph = Paragraph::new(lines).block(block);
    f.render_widget(paragraph, area);
}

/// Render context-sensitive control hints at the bottom.
fn render_hints(f: &mut Frame, area: Rect, browser: &DirectoryBrowser, variant: &BrowserVariant) {
    let BrowserVariant::CorpusBrowser(ref v) = variant;

    let hints = if v.dir_config_panel.is_some() {
        Line::from(vec![
            control_colors::nav("^v"),
            control_colors::text(" nav  "),
            control_colors::confirm("Enter"),
            control_colors::text(" edit  "),
            control_colors::cancel("Esc"),
            control_colors::text(" close panel"),
        ])
    } else {
        let cursor_entry = browser.current_entry();
        // All entries in this browser are corpus entries by construction (zone-filtered query).
        let on_corpus_dir = cursor_entry
            .map(|e| e.is_dir)
            .unwrap_or(false);
        let on_dir = cursor_entry.map(|e| e.is_dir).unwrap_or(false);

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

        let on_packing = cursor_entry
            .map(|e| v.packing_marker_for(&e.path, e.is_dir) != PackingMarker::None)
            .unwrap_or(false);
        if on_packing {
            spans.push(control_colors::text("  "));
            spans.push(control_colors::edit("Z"));
            spans.push(control_colors::text(" info"));
        }

        let has_packing_dirs = !v.packing_dir_categories().is_empty();
        if has_packing_dirs {
            spans.push(control_colors::text("  "));
            spans.push(control_colors::nav("Tab"));
            spans.push(control_colors::text(" next MB"));
        }

        Line::from(spans)
    };

    f.render_widget(Paragraph::new(hints), area);
}

/// Render a single tree entry line using BrowserEntry + computed markers.
fn render_entry_line(
    entry: &BrowserEntry,
    is_cursor: bool,
    is_pending_edit: bool,
    deploy_marker: DeployMarker,
    packing_marker: PackingMarker,
    is_dimmed: bool,
) -> Line<'static> {
    let indent = "  ".repeat(entry.depth);

    let expand_indicator = if entry.is_dir {
        if entry.expanded {
            "▽ "
        } else {
            "▷ "
        }
    } else {
        "  "
    };

    let icon = if entry.is_dir {
        ""
    } else {
        "♪ "
    };

    let count_suffix = if entry.is_dir && entry.file_count > 0 {
        format!("  ({} tracks)", entry.file_count)
    } else {
        String::new()
    };

    let (deploy_suffix, deploy_style) = match deploy_marker {
        DeployMarker::SourceRoot => ("  [D]", Style::default().fg(Color::Magenta)),
        DeployMarker::Inherited => ("  [d]", Style::default().fg(Color::DarkGray)),
        DeployMarker::None => ("", Style::default()),
    };

    let base_style = if is_cursor {
        CURSOR_STYLE
    } else if is_dimmed {
        Style::default().fg(Color::DarkGray)
    } else if entry.is_dir {
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

    match &packing_marker {
        PackingMarker::None => {}
        PackingMarker::Matched => {
            spans.push(Span::styled("  [MB]", Style::default().fg(Color::Green)));
        }
        PackingMarker::Directory(cat) => {
            spans.push(Span::styled(
                format!("  [MB{}]", cat.marker_symbol()),
                Style::default().fg(cat.color()),
            ));
        }
    }

    if is_pending_edit {
        spans.push(Span::styled("  [*]", pending_style));
    }

    Line::from(spans)
}
