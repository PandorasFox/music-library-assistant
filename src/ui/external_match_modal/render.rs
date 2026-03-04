//! Rendering for the external match review modal.
//!
//! Read-only browser: track → MB recording URL + confidence.
//! Uses full-area layout with:
//! - Info bar showing full untruncated path and confidence
//! - 40% list pane / 60% details pane (MB recording info)

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::ui::helpers::render_pane;
use crate::ui::widgets::{render_file_path_list, PathEntry, PathField, ResolutionLayout};

use super::types::ExternalMatchReviewState;

pub fn render(f: &mut Frame, area: Rect, state: &mut ExternalMatchReviewState) {
    let padded = ResolutionLayout::padded(area);
    f.render_widget(Clear, padded);

    // Two-row layout: info bar (3 lines) + content panes (rest)
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(5),
        ])
        .split(padded);

    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(40),
            Constraint::Percentage(60),
        ])
        .split(vertical[1]);

    render_info_bar(f, vertical[0], state);
    render_file_list(f, horizontal[0], state);
    render_details(f, horizontal[1], state);

    // Recording detail overlay (covers the details pane)
    if state.viewing_detail.is_some() {
        render_recording_detail(f, padded, state);
    }
}

fn render_info_bar(f: &mut Frame, area: Rect, state: &ExternalMatchReviewState) {
    let title = format!(
        " External Match Browser \u{2014} {} file{} ",
        state.entries.len(),
        if state.entries.len() == 1 { "" } else { "s" },
    );

    let block = Block::default()
        .title(title)
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));

    let inner = render_pane(f, area, block);

    if let Some(entry) = state.entries.get(state.cursor) {
        let lines = PathField::new(
            Span::styled("Path: ", Style::default().fg(Color::DarkGray)),
            &entry.path,
        )
        .style(Style::default().fg(Color::White))
        .render_lines(inner.width);
        let para = Paragraph::new(lines);
        f.render_widget(para, inner);
    }
}

fn render_file_list(f: &mut Frame, area: Rect, state: &mut ExternalMatchReviewState) {
    let block = Block::default()
        .title("Files")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));
    let inner = render_pane(f, area, block);

    state.click_targets.clear();
    state.click_targets.set_list_area(inner);
    let visible_height = inner.height as usize;
    let scroll = state.scroll;
    for (vis_idx, entry_idx) in (scroll..).take(visible_height).enumerate() {
        if entry_idx >= state.entries.len() { break; }
        state.click_targets.add_row(entry_idx.to_string(), inner.y + vis_idx as u16);
    }

    let entries: Vec<PathEntry> = state.entries
        .iter()
        .map(|entry| {
            PathEntry {
                path: &entry.path,
                prefix: vec![],
                suffix: vec![
                    Span::styled(
                        format!(" {:.0}%", entry.confidence * 100.0),
                        Style::default().fg(Color::DarkGray),
                    ),
                ],
            }
        })
        .collect();

    render_file_path_list(f, inner, &entries, state.cursor, state.scroll);
}

fn render_details(f: &mut Frame, area: Rect, state: &mut ExternalMatchReviewState) {
    let block = Block::default()
        .title("Recording")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = render_pane(f, area, block);

    let Some(entry) = state.entries.get(state.cursor) else {
        state.recording_link_rect = None;
        return;
    };

    let label_style = Style::default().fg(Color::DarkGray);
    let value_style = Style::default().fg(Color::White);
    let dim_style = Style::default().fg(Color::DarkGray);

    let mut lines = Vec::new();

    // Confidence
    lines.push(Line::from(vec![
        Span::styled("Confidence: ", label_style),
        Span::styled(
            format!("{:.1}%", entry.confidence * 100.0),
            Style::default().fg(Color::Yellow),
        ),
    ]));

    // MusicBrainz recording URL
    let mb_url = format!("https://musicbrainz.org/recording/{}", entry.recording_id);
    let link_style = Style::default()
        .fg(Color::Blue)
        .add_modifier(Modifier::UNDERLINED);
    let link_len = mb_url.chars().count() as u16;

    let link_x = inner.x;
    let link_y = inner.y + 1;
    state.recording_link_rect = Some(Rect::new(link_x, link_y, link_len, 1));

    lines.push(Line::from(Span::styled(&mb_url, link_style)));
    lines.push(Line::raw(""));

    // Inline MB recording summary if pre-loaded
    if let Some(summary) = state.recording_summaries.get(&entry.recording_id) {
        lines.push(Line::from(vec![
            Span::styled("Title:  ", label_style),
            Span::styled(&summary.title, value_style),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Artist: ", label_style),
            Span::styled(&summary.artist_credit, value_style),
        ]));
        if let Some(length_ms) = summary.length_ms {
            let mins = length_ms / 60000;
            let secs = (length_ms % 60000) / 1000;
            lines.push(Line::from(vec![
                Span::styled("Length: ", label_style),
                Span::styled(format!("{}:{:02}", mins, secs), value_style),
            ]));
        }
        if summary.release_count > 0 {
            lines.push(Line::from(vec![
                Span::styled("Releases: ", label_style),
                Span::styled(
                    format!("{}", summary.release_count),
                    value_style,
                ),
            ]));
        }
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            "Press 'v' for full recording detail.",
            dim_style,
        )));
    } else {
        lines.push(Line::from(Span::styled(
            "MB recording data not cached.",
            dim_style,
        )));
        lines.push(Line::from(Span::styled(
            "Run a fetch to populate.",
            dim_style,
        )));
    }

    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "Esc to close  \u{2191}\u{2193} navigate  click link to open",
        dim_style,
    )));

    // Apply scroll
    let visible: Vec<Line> = lines.into_iter().skip(state.detail_scroll).collect();
    let para = Paragraph::new(visible);
    f.render_widget(para, inner);
}

fn render_recording_detail(f: &mut Frame, area: Rect, state: &ExternalMatchReviewState) {
    let detail = match state.viewing_detail {
        Some(ref d) => d,
        None => return,
    };

    // Clear and draw bordered box over the whole area
    f.render_widget(Clear, area);
    let block = Block::default()
        .title(" Recording Detail ")
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    let inner = render_pane(f, area, block);

    let label_style = Style::default().fg(Color::DarkGray);
    let value_style = Style::default().fg(Color::White);
    let heading_style = Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD);
    let dim_style = Style::default().fg(Color::DarkGray);

    let mut lines: Vec<Line> = Vec::new();

    // Recording title + length
    let rec = &detail.recording;
    lines.push(Line::from(vec![
        Span::styled("Recording: ", label_style),
        Span::styled(format!("\"{}\"", rec.title), value_style),
    ]));
    if let Some(length_ms) = rec.length {
        let mins = length_ms / 60000;
        let secs = (length_ms % 60000) / 1000;
        lines.push(Line::from(vec![
            Span::styled("Length:    ", label_style),
            Span::styled(format!("{}:{:02}", mins, secs), value_style),
        ]));
    }
    lines.push(Line::from(vec![
        Span::styled("MBID:      ", label_style),
        Span::styled(&rec.id, dim_style),
    ]));
    lines.push(Line::raw(""));

    // Artist credits
    if !rec.artist_credit.is_empty() {
        lines.push(Line::from(Span::styled("Artist Credits:", heading_style)));
        for credit in &rec.artist_credit {
            let mut spans = vec![
                Span::styled("  ", label_style),
                Span::styled(&credit.name, value_style),
            ];
            if credit.artist.sort_name != credit.artist.name {
                spans.push(Span::styled(
                    format!(" (sort: {})", credit.artist.sort_name),
                    dim_style,
                ));
            }
            if !credit.joinphrase.is_empty() {
                spans.push(Span::styled(
                    format!(" [join: \"{}\"]", credit.joinphrase.trim()),
                    dim_style,
                ));
            }
            lines.push(Line::from(spans));

            // Show aliases if we have cached artist data
            if let Some((_, Some(ref artist))) = detail.artists.iter()
                .find(|(id, _)| *id == credit.artist.id)
            {
                if artist.name != credit.name {
                    lines.push(Line::from(vec![
                        Span::styled("    canonical: ", dim_style),
                        Span::styled(&artist.name, dim_style),
                        Span::styled(format!(" [{}]", artist.id), dim_style),
                    ]));
                }
                if artist.sort_name != artist.name {
                    lines.push(Line::from(vec![
                        Span::styled("    sort: ", dim_style),
                        Span::styled(&artist.sort_name, dim_style),
                    ]));
                }
                if !artist.aliases.is_empty() {
                    let alias_strs: Vec<String> = artist.aliases.iter()
                        .take(5)
                        .map(|a| {
                            let mut s = a.name.clone();
                            if let Some(ref locale) = a.locale {
                                s = format!("{} ({})", s, locale);
                            }
                            if let Some(ref t) = a.type_ {
                                if a.primary.as_deref() == Some("primary") {
                                    s = format!("{} [{}*]", s, t);
                                }
                            }
                            s
                        })
                        .collect();
                    lines.push(Line::from(vec![
                        Span::styled("    aka: ", dim_style),
                        Span::styled(alias_strs.join(", "), dim_style),
                    ]));
                }
            }
        }
        lines.push(Line::raw(""));
    }

    // Relations
    let relevant_relations: Vec<_> = rec.relations.iter()
        .filter(|r| r.artist.is_some() && r.direction.as_deref() != Some("forward"))
        .collect();
    if !relevant_relations.is_empty() {
        lines.push(Line::from(Span::styled("Relations:", heading_style)));
        for relation in &relevant_relations {
            if let Some(ref artist) = relation.artist {
                let mut role = relation.type_.clone();
                if !relation.attributes.is_empty() {
                    role = format!("{} ({})", role, relation.attributes.join(", "));
                }
                lines.push(Line::from(vec![
                    Span::styled("  ", label_style),
                    Span::styled(format!("{}: ", role), dim_style),
                    Span::styled(&artist.name, value_style),
                ]));
            }
        }
        lines.push(Line::raw(""));
    }

    // Releases
    if !detail.releases.is_empty() {
        lines.push(Line::from(Span::styled("Releases:", heading_style)));
        for (id, parsed) in &detail.releases {
            match parsed {
                Some(release) => {
                    let artist_str: String = release.artist_credit.iter()
                        .map(|c| c.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ");
                    let mut spans = vec![
                        Span::styled("  ", label_style),
                        Span::styled(&release.title, value_style),
                    ];
                    if !artist_str.is_empty() {
                        spans.push(Span::styled(format!(" by {}", artist_str), dim_style));
                    }
                    lines.push(Line::from(spans));
                    lines.push(Line::from(vec![
                        Span::styled("    ", label_style),
                        Span::styled(&release.id, dim_style),
                    ]));
                }
                None => {
                    let fallback_title = rec.releases.iter()
                        .find(|r| r.id == *id)
                        .and_then(|r| r.title.as_deref());
                    let fallback_rg = rec.releases.iter()
                        .find(|r| r.id == *id)
                        .and_then(|r| r.release_group.as_ref());
                    let mut spans = vec![
                        Span::styled("  ", label_style),
                    ];
                    if let Some(title) = fallback_title {
                        spans.push(Span::styled(title, value_style));
                        spans.push(Span::styled(" (not cached)", dim_style));
                    } else {
                        spans.push(Span::styled(id, dim_style));
                        spans.push(Span::styled(" (not cached)", dim_style));
                    }
                    lines.push(Line::from(spans));
                    if let Some(rg) = fallback_rg {
                        lines.push(Line::from(vec![
                            Span::styled("    release-group: ", dim_style),
                            Span::styled(&rg.id, dim_style),
                        ]));
                    }
                }
            }
        }
        lines.push(Line::raw(""));
    }

    // Hint line
    lines.push(Line::from(Span::styled(
        "Esc to close  \u{2191}\u{2193} to scroll",
        dim_style,
    )));

    // Apply scroll
    let scroll = detail.scroll;
    let visible: Vec<Line> = lines.into_iter().skip(scroll).collect();

    let para = Paragraph::new(visible);
    f.render_widget(para, inner);
}
