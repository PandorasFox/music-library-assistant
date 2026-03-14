//! Tag Canonicity V3 — ModalFrame impl for the packed single-load state.
//!
//! `TagCanonicityState` implements `ModalFrame` to render outlier variants
//! as list items and files for the selected variant as detail. The
//! `DecisionField` rendering is handled by the blanket
//! `ModalFrame for WithDecisionField<TagCanonicityState>` impl.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{ListItem, Paragraph};
use ratatui::Frame;

use mm_ui::group_navigation::GroupNavigation;
use mm_ui::resolutions::tag_canonicity::TagCanonicityState;

use crate::helpers::truncate_right;
use crate::widgets::modal_frame::ModalFrame;

impl ModalFrame for TagCanonicityState {
    fn frame_title(&self) -> Line<'static> {
        let current = self.data.current_cluster + 1;
        let total = self.data.group_count();
        let tag = &self.data.inner.tag_name;
        Line::styled(
            format!(" Squash \"{}\" variants ({}/{}) ", tag, current, total),
            Style::default().fg(Color::Cyan),
        )
    }

    fn accent_color(&self) -> Color {
        Color::Cyan
    }

    fn render_list_item(
        &self,
        idx: usize,
        width: u16,
        is_cursor: bool,
        is_focused: bool,
    ) -> ListItem<'static> {
        let cluster = match self.data.inner.clusters.get(self.data.current_cluster) {
            Some(c) => c,
            None => return ListItem::new(""),
        };
        let variant = match cluster.outlier_variants.get(idx) {
            Some(v) => v,
            None => return ListItem::new(""),
        };

        let file_count = variant.files.len();
        let suffix = if file_count == 1 { "file" } else { "files" };

        let max_width = width.saturating_sub(2) as usize;
        let value_max = max_width.saturating_sub(15);
        let display = truncate_right(&variant.value, value_max);
        let text = format!("\"{}\" ({} {})", display, file_count, suffix);

        let style = if is_cursor && is_focused {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else if is_cursor {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::White)
        };

        ListItem::new(Span::styled(text, style))
    }

    fn render_detail(&mut self, f: &mut Frame, area: Rect) {
        let cluster = match self.data.inner.clusters.get(self.data.current_cluster) {
            Some(c) => c,
            None => return,
        };
        let variant = match cluster.outlier_variants.get(self.cursor) {
            Some(v) => v,
            None => {
                let info = format!(
                    "Canonical: \"{}\" ({} files)",
                    cluster.canonical_candidate, cluster.canonical_count,
                );
                f.render_widget(
                    Paragraph::new(info).style(Style::default().fg(Color::DarkGray)),
                    area,
                );
                return;
            }
        };

        let max_width = area.width.saturating_sub(2) as usize;
        let visible = area.height as usize;

        let lines: Vec<Line> = variant
            .files
            .iter()
            .take(visible)
            .map(|file| {
                let name = truncate_right(&file.display_name, max_width);
                Line::from(Span::styled(name, Style::default().fg(Color::White)))
            })
            .collect();

        if lines.is_empty() {
            f.render_widget(
                Paragraph::new("No files").style(Style::default().fg(Color::DarkGray)),
                area,
            );
        } else {
            f.render_widget(Paragraph::new(lines), area);
        }
    }

    fn render_header(&self, f: &mut Frame, area: Rect) {
        let cluster = match self.data.inner.clusters.get(self.data.current_cluster) {
            Some(c) => c,
            None => return,
        };
        let info = format!(
            "Canonical: \"{}\" ({} files) — {} outlier variant(s)",
            cluster.canonical_candidate,
            cluster.canonical_count,
            cluster.outlier_variants.len(),
        );
        f.render_widget(
            Paragraph::new(info).style(Style::default().fg(Color::DarkGray)),
            area,
        );
    }
}
