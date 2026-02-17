//! Path display helpers for wrapping long paths at `/` boundaries.
//!
//! Provides:
//! - [`wrap_path`] — core greedy line-breaking algorithm
//! - [`PathField`] — labeled path widget for detail panes
//! - [`path_lines`] — standalone wrapped path (no label/indent)

use ratatui::{
    style::Style,
    text::{Line, Span},
};

/// Split a path into display segments at `/` boundaries.
///
/// Each segment fits within `budget` characters. Greedy: packs as many
/// path components as possible per line, breaking before `/` so that
/// continuation lines start with `/`.
///
/// If a single component exceeds `budget`, it is hard-broken at `budget`.
pub fn wrap_path(path: &str, budget: usize) -> Vec<String> {
    if budget == 0 {
        return vec![path.to_string()];
    }

    let char_count = path.chars().count();
    if char_count <= budget {
        return vec![path.to_string()];
    }

    let chars: Vec<char> = path.chars().collect();
    let mut segments = Vec::new();
    let mut start = 0;

    while start < chars.len() {
        let remaining = chars.len() - start;
        if remaining <= budget {
            // Rest fits in one line
            segments.push(chars[start..].iter().collect());
            break;
        }

        // Find last '/' within budget from `start`
        let end = start + budget;
        let window = &chars[start..end];

        let break_at = window.iter().rposition(|&c| c == '/');

        match break_at {
            Some(0) if budget == 1 => {
                // Edge case: budget is 1 and the first char is '/';
                // hard-break to avoid infinite loop.
                segments.push("/".to_string());
                start += 1;
            }
            Some(pos) if pos > 0 => {
                // Break *before* the slash so the next segment starts with '/'
                segments.push(chars[start..start + pos].iter().collect());
                start += pos;
            }
            _ => {
                // No '/' in window — hard-break at budget
                segments.push(chars[start..end].iter().collect());
                start = end;
            }
        }
    }

    segments
}

/// A labeled path field that wraps at `/` boundaries.
///
/// Continuation lines are indented to align with the path start
/// (i.e., past the label).
///
/// ```text
/// Path: /very/long/path/to/some/deeply
///       /nested/directory/with/music
///       /Artist - Album/01 Song.flac
/// ```
pub struct PathField<'a> {
    label: Span<'a>,
    path: &'a str,
    path_style: Style,
}

impl<'a> PathField<'a> {
    pub fn new(label: Span<'a>, path: &'a str) -> Self {
        Self {
            label,
            path,
            path_style: Style::default(),
        }
    }

    pub fn style(mut self, style: Style) -> Self {
        self.path_style = style;
        self
    }

    /// Produce the wrapped lines ready for inclusion in a `Vec<Line>`.
    ///
    /// All output data is owned (`'static`) — the label and path segments
    /// are cloned/converted to owned strings.
    pub fn render_lines(&self, width: u16) -> Vec<Line<'static>> {
        let label_width = self.label.content.chars().count();
        let budget = (width as usize).saturating_sub(label_width);
        let owned_label = Span::styled(self.label.content.to_string(), self.label.style);

        if budget == 0 {
            // No room for the path at all; just show label + path unsplit
            return vec![Line::from(vec![
                owned_label,
                Span::styled(self.path.to_string(), self.path_style),
            ])];
        }

        let segments = wrap_path(self.path, budget);
        let indent: String = " ".repeat(label_width);

        segments
            .into_iter()
            .enumerate()
            .map(|(i, seg)| {
                if i == 0 {
                    Line::from(vec![
                        owned_label.clone(),
                        Span::styled(seg, self.path_style),
                    ])
                } else {
                    Line::from(vec![
                        Span::raw(indent.clone()),
                        Span::styled(seg, self.path_style),
                    ])
                }
            })
            .collect()
    }
}

/// Produce wrapped path lines with no label or indent.
///
/// Useful when the label is on a separate line above the path.
pub fn path_lines(path: &str, style: Style, width: u16) -> Vec<Line<'static>> {
    let budget = width as usize;
    if budget == 0 {
        return vec![Line::from(Span::styled(path.to_string(), style))];
    }

    wrap_path(path, budget)
        .into_iter()
        .map(|seg| Line::from(Span::styled(seg, style)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_fits_budget() {
        let result = wrap_path("/short/path.flac", 40);
        assert_eq!(result, vec!["/short/path.flac"]);
    }

    #[test]
    fn multi_segment_wrap_at_slash() {
        let result = wrap_path("/very/long/path/to/file.flac", 15);
        // Should break at '/' boundaries
        for seg in &result {
            assert!(
                seg.chars().count() <= 15,
                "segment too long: {:?} ({} chars)",
                seg,
                seg.chars().count()
            );
        }
        // Reassembled should equal original
        let reassembled: String = result.concat();
        assert_eq!(reassembled, "/very/long/path/to/file.flac");
    }

    #[test]
    fn hard_break_when_component_exceeds_budget() {
        // Single component with no slashes that exceeds budget
        let result = wrap_path("averylongfilename.flac", 10);
        assert!(result.len() > 1);
        for seg in &result[..result.len() - 1] {
            assert_eq!(seg.chars().count(), 10);
        }
        let reassembled: String = result.concat();
        assert_eq!(reassembled, "averylongfilename.flac");
    }

    #[test]
    fn unicode_paths() {
        let path = "/音楽/アーティスト/アルバム/曲.flac";
        let budget = 12;
        let result = wrap_path(path, budget);
        for seg in &result {
            assert!(
                seg.chars().count() <= budget,
                "segment too long: {:?} ({} chars)",
                seg,
                seg.chars().count()
            );
        }
        let reassembled: String = result.concat();
        assert_eq!(reassembled, path);
    }

    #[test]
    fn empty_budget_returns_whole_path() {
        let result = wrap_path("/some/path", 0);
        assert_eq!(result, vec!["/some/path"]);
    }

    #[test]
    fn budget_of_one() {
        let result = wrap_path("/a/b", 1);
        let reassembled: String = result.concat();
        assert_eq!(reassembled, "/a/b");
    }

    #[test]
    fn exact_fit() {
        let path = "/exact/fit";
        let result = wrap_path(path, path.len());
        assert_eq!(result, vec![path]);
    }

    #[test]
    fn path_field_indentation_alignment() {
        let label = Span::raw("Path: ");
        let path = "/very/long/path/to/some/deeply/nested/directory/file.flac";
        let field = PathField::new(label, path);
        let lines = field.render_lines(30);

        // First line should start with label
        assert_eq!(lines[0].spans[0].content, "Path: ");

        // Continuation lines should be indented with spaces matching label width
        if lines.len() > 1 {
            let indent = &lines[1].spans[0];
            assert_eq!(indent.content.chars().count(), 6); // "Path: " is 6 chars
        }
    }

    #[test]
    fn path_lines_no_indent() {
        let lines = path_lines("/very/long/path/to/file.flac", Style::default(), 15);
        assert!(lines.len() > 1);
        // No line should have leading whitespace indent (unlike PathField)
        for line in &lines {
            assert_eq!(line.spans.len(), 1);
        }
    }
}
