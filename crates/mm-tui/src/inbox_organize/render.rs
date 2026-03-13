//! Rendering for the inbox organize workflow.
//!
//! Layout:
//! ```text
//! ┌─ Organize Inbox → Corpus (1/5) ──────────────────────────────┐
//! │  ┌─ inbox/ArtistA/ ──────────┐  ┌─ corpus ─────────────────┐ │
//! │  │  01 - Track One.flac      │  │  [+ new directory]        │ │
//! │  │  02 - Track Two.flac      │  │  ▶ ArtistA               │ │
//! │  └───────────────────────────┘  └───────────────────────────┘ │
//! │  ↑↓ navigate  ←→ expand/collapse  Enter select  S skip  Esc ⌧│
//! └──────────────────────────────────────────────────────────────┘
//! ```

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

use crate::widgets::{control_colors as cc, CURSOR_STYLE};

use super::{EmplaceOption, InboxOrganizeState, OrganizePhase};

/// Render the inbox organize view.
pub fn render(f: &mut Frame, area: Rect, state: &mut InboxOrganizeState) {
    // Main layout: header + content + controls
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Header
            Constraint::Min(5),    // Content (side-by-side panes)
            Constraint::Length(1), // Controls hint
        ])
        .split(area);

    render_header(f, main_chunks[0], state);
    render_content(f, main_chunks[1], state);
    render_controls(f, main_chunks[2], state);

    // Overlays
    match state.phase {
        OrganizePhase::EmplacePopup => render_emplace_popup(f, area, state),
        OrganizePhase::NewDirectoryInput => render_new_dir_input(f, area, state),
        OrganizePhase::BrowsingCorpus => {}
    }
}

/// Render the header bar.
fn render_header(f: &mut Frame, area: Rect, state: &InboxOrganizeState) {
    let title = format!(" Organize Inbox → Corpus ({}) ", state.progress_label());
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Green))
        .title(title)
        .title_alignment(Alignment::Center);
    f.render_widget(block, area);
}

/// Render the side-by-side content panes.
fn render_content(f: &mut Frame, area: Rect, state: &mut InboxOrganizeState) {
    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(35), // Inbox directory listing
            Constraint::Percentage(65), // Corpus tree browser
        ])
        .split(area);

    render_inbox_pane(f, panes[0], state);
    render_corpus_pane(f, panes[1], state);
}

/// Render the inbox directory file listing (left pane, read-only).
fn render_inbox_pane(f: &mut Frame, area: Rect, state: &InboxOrganizeState) {
    let title = match state.current_dir() {
        Some(dir) => format!(" inbox/{} ", dir.dir_name),
        None => " (done) ".to_string(),
    };

    let lines: Vec<Line> = state
        .current_dir()
        .map(|dir| {
            dir.files
                .iter()
                .map(|file| {
                    Line::from(Span::styled(
                        format!("  {}", file.filename),
                        Style::default().fg(Color::White),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow))
        .title(title);
    let paragraph = Paragraph::new(lines).block(block);
    f.render_widget(paragraph, area);
}

/// Render the corpus tree browser (right pane, interactive).
fn render_corpus_pane(f: &mut Frame, area: Rect, state: &mut InboxOrganizeState) {
    let inner_height = area.height.saturating_sub(2) as usize;
    state.corpus_navigator.set_visible_height(inner_height);

    let entries = state.corpus_navigator.entries();
    let cursor_idx = state.corpus_navigator.cursor_idx();
    let scroll = state.corpus_navigator.scroll_offset();

    let lines: Vec<Line> = entries
        .iter()
        .enumerate()
        .skip(scroll)
        .take(inner_height)
        .map(|(idx, entry)| render_corpus_entry(entry, idx == cursor_idx))
        .collect();

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(format!(" corpus [{}/{}] ", cursor_idx + 1, entries.len()));
    let paragraph = Paragraph::new(lines).block(block);
    f.render_widget(paragraph, area);
}

/// Render a single corpus tree entry line.
fn render_corpus_entry(
    entry: &crate::tree_browser::TreeEntry,
    is_cursor: bool,
) -> Line<'static> {
    let indent = "  ".repeat(entry.depth);

    if entry.is_synthetic {
        // Render "[+ new directory]" in distinctive style
        let style = if is_cursor {
            CURSOR_STYLE
        } else {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::ITALIC)
        };
        return Line::from(vec![
            Span::raw(indent),
            Span::styled("  ", Style::default()),
            Span::styled(entry.name.clone(), style),
        ]);
    }

    let expand_indicator = if entry.is_directory() {
        if entry.has_children {
            if entry.is_expanded {
                "▽ "
            } else {
                "▷ "
            }
        } else {
            "  "
        }
    } else {
        "  "
    };

    let base_style = if is_cursor {
        CURSOR_STYLE
    } else if entry.is_directory() {
        Style::default().fg(Color::Blue)
    } else {
        Style::default().fg(Color::White)
    };

    let expand_style = Style::default().fg(Color::Yellow);

    Line::from(vec![
        Span::raw(indent),
        Span::styled(expand_indicator, expand_style),
        Span::styled(entry.name.clone(), base_style),
    ])
}

/// Render the controls hint bar.
fn render_controls(f: &mut Frame, area: Rect, _state: &InboxOrganizeState) {
    let hints = Line::from(vec![
        cc::nav(" ↑↓"),
        cc::text(" navigate  "),
        cc::nav("←→"),
        cc::text(" expand  "),
        cc::confirm("[Enter]"),
        cc::text(" select  "),
        cc::action("[S]"),
        cc::text(" skip  "),
        cc::cancel("[Esc]"),
        cc::text(" cancel"),
    ]);
    f.render_widget(Paragraph::new(hints), area);
}

/// Render the emplace confirmation popup.
fn render_emplace_popup(f: &mut Frame, area: Rect, state: &InboxOrganizeState) {
    let popup = centered_popup(area, 50, 10);
    f.render_widget(Clear, popup);

    let source_display = state
        .current_dir()
        .map(|d| d.dir_path.to_string_lossy().to_string())
        .unwrap_or_else(|| "?".to_string());

    let root = state.corpus_navigator.root_path();
    let root_name = root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "corpus".to_string());
    let dest_display = match state.selected_dest.strip_prefix(root) {
        Ok(rel) if rel.as_os_str().is_empty() => root_name,
        Ok(rel) => format!("{}/{}", root_name, rel.display()),
        Err(_) => state.selected_dest.to_string_lossy().to_string(),
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Green))
        .title(" How do you want to organize? ")
        .title_alignment(Alignment::Center);

    let inner = block.inner(popup);
    f.render_widget(block, popup);

    // Content lines
    let content_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // "Moving:" line
            Constraint::Length(1), // "Into:" line
            Constraint::Length(1), // spacer
            Constraint::Length(1), // options row
            Constraint::Length(1), // hint
        ])
        .split(inner);

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("  Moving: ", Style::default().fg(Color::DarkGray)),
            Span::styled(source_display, Style::default().fg(Color::Yellow)),
        ])),
        content_chunks[0],
    );

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("  Into:   ", Style::default().fg(Color::DarkGray)),
            Span::styled(dest_display, Style::default().fg(Color::Cyan)),
        ])),
        content_chunks[1],
    );

    // Options row
    let options = [
        EmplaceOption::EmplaceDirectory,
        EmplaceOption::EmplaceFiles,
        EmplaceOption::Skip,
        EmplaceOption::Cancel,
    ];

    let option_spans: Vec<Span> = options
        .iter()
        .flat_map(|opt| {
            let is_selected = *opt == state.popup_selection;
            let style = if is_selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Green)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            vec![
                Span::styled(format!(" [{}] ", opt.label()), style),
                Span::raw(" "),
            ]
        })
        .collect();

    f.render_widget(
        Paragraph::new(Line::from(option_spans)).alignment(Alignment::Center),
        content_chunks[3],
    );

    // Hint
    f.render_widget(
        Paragraph::new(Line::from(vec![
            cc::nav("←/→"),
            cc::text(" switch  "),
            cc::confirm("[Enter]"),
            cc::text(" confirm  "),
            cc::cancel("[Esc]"),
            cc::text(" back"),
        ]))
        .alignment(Alignment::Center),
        content_chunks[4],
    );
}

/// Render the new directory name input popup.
fn render_new_dir_input(f: &mut Frame, area: Rect, state: &InboxOrganizeState) {
    let popup = centered_popup(area, 50, 7);
    f.render_widget(Clear, popup);

    let parent_name = state
        .selected_dest
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "corpus".to_string());

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(format!(" New directory under {} ", parent_name))
        .title_alignment(Alignment::Center);

    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Label
            Constraint::Length(1), // Input
            Constraint::Length(1), // Hint
        ])
        .split(inner);

    f.render_widget(
        Paragraph::new(Span::styled(
            "  Directory name:",
            Style::default().fg(Color::DarkGray),
        )),
        chunks[0],
    );

    // Render text input with cursor
    let value = state.new_dir_input.value();
    let cursor_pos = state.new_dir_input.cursor;
    let mut spans = vec![Span::raw("  ")];
    if value.is_empty() {
        spans.push(Span::styled(
            "_",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::SLOW_BLINK),
        ));
    } else {
        let chars: Vec<char> = value.chars().collect();
        let before: String = chars[..cursor_pos].iter().collect();
        let cursor_char = chars.get(cursor_pos).copied().unwrap_or(' ');
        let after: String = if cursor_pos < chars.len() {
            chars[cursor_pos + 1..].iter().collect()
        } else {
            String::new()
        };

        spans.push(Span::styled(before, Style::default().fg(Color::White)));
        spans.push(Span::styled(
            cursor_char.to_string(),
            Style::default().fg(Color::Black).bg(Color::White),
        ));
        spans.push(Span::styled(after, Style::default().fg(Color::White)));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), chunks[1]);

    f.render_widget(
        Paragraph::new(Line::from(vec![
            cc::confirm("  [Enter]"),
            cc::text(" create  "),
            cc::cancel("[Esc]"),
            cc::text(" cancel"),
        ])),
        chunks[2],
    );
}

/// Compute a centered popup rect within the given area.
fn centered_popup(area: Rect, width_pct: u16, height: u16) -> Rect {
    let vert = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Fill(1),
            Constraint::Length(height),
            Constraint::Fill(1),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - width_pct) / 2),
            Constraint::Percentage(width_pct),
            Constraint::Percentage((100 - width_pct) / 2),
        ])
        .split(vert[1])[1]
}
