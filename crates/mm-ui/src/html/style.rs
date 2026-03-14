//! ratatui Color/Style → CSS mapping.
//!
//! Uses CSS custom properties (`var(--c-green)`) so themes are swappable
//! via a single stylesheet. The property names match the CSS variables
//! defined in `mm-web/static/mm.css`.

use ratatui::style::{Color, Modifier, Style};

/// Map a ratatui `Color` to a CSS value string.
///
/// Named colors map to CSS custom properties. RGB/indexed map to direct values.
pub fn color_to_css(color: Color) -> String {
    match color {
        Color::Reset => String::new(),
        Color::Black => "var(--c-black)".into(),
        Color::Red => "var(--c-red)".into(),
        Color::Green => "var(--c-green)".into(),
        Color::Yellow => "var(--c-yellow)".into(),
        Color::Blue => "var(--c-blue)".into(),
        Color::Magenta => "var(--c-magenta)".into(),
        Color::Cyan => "var(--c-cyan)".into(),
        Color::Gray => "var(--c-gray)".into(),
        Color::DarkGray => "var(--c-dark-gray)".into(),
        Color::LightRed => "var(--c-light-red)".into(),
        Color::LightGreen => "var(--c-light-green)".into(),
        Color::LightYellow => "var(--c-light-yellow)".into(),
        Color::LightBlue => "var(--c-light-blue)".into(),
        Color::LightMagenta => "var(--c-light-magenta)".into(),
        Color::LightCyan => "var(--c-light-cyan)".into(),
        Color::White => "var(--c-white)".into(),
        Color::Rgb(r, g, b) => format!("rgb({r},{g},{b})"),
        Color::Indexed(idx) => format!("var(--c-idx-{idx})"),
    }
}

/// Convert a ratatui `Style` to an inline CSS string.
///
/// Only emits properties that are actually set. Returns empty string for
/// a default/unstyled style.
pub fn style_to_inline_css(style: &Style) -> String {
    let mut parts = Vec::new();

    if let Some(fg) = style.fg {
        let css = color_to_css(fg);
        if !css.is_empty() {
            parts.push(format!("color:{css}"));
        }
    }

    if let Some(bg) = style.bg {
        let css = color_to_css(bg);
        if !css.is_empty() {
            parts.push(format!("background-color:{css}"));
        }
    }

    let mods = style.add_modifier;
    if mods.contains(Modifier::BOLD) {
        parts.push("font-weight:bold".into());
    }
    if mods.contains(Modifier::ITALIC) {
        parts.push("font-style:italic".into());
    }
    if mods.contains(Modifier::UNDERLINED) {
        parts.push("text-decoration:underline".into());
    }
    if mods.contains(Modifier::DIM) {
        parts.push("opacity:0.6".into());
    }
    if mods.contains(Modifier::CROSSED_OUT) {
        parts.push("text-decoration:line-through".into());
    }

    parts.join(";")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_colors() {
        assert_eq!(color_to_css(Color::Green), "var(--c-green)");
        assert_eq!(color_to_css(Color::Red), "var(--c-red)");
        assert_eq!(color_to_css(Color::White), "var(--c-white)");
    }

    #[test]
    fn rgb_color() {
        assert_eq!(color_to_css(Color::Rgb(255, 128, 0)), "rgb(255,128,0)");
    }

    #[test]
    fn default_style_empty() {
        assert_eq!(style_to_inline_css(&Style::default()), "");
    }

    #[test]
    fn fg_and_bold() {
        let style = Style::default().fg(Color::Green).add_modifier(Modifier::BOLD);
        assert_eq!(
            style_to_inline_css(&style),
            "color:var(--c-green);font-weight:bold"
        );
    }

    #[test]
    fn bg_only() {
        let style = Style::default().bg(Color::DarkGray);
        assert_eq!(
            style_to_inline_css(&style),
            "background-color:var(--c-dark-gray)"
        );
    }
}
