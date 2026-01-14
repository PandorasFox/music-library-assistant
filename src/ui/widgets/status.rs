//! Status Color and Health Indicator Widgets
//!
//! Consistent color coding for health, status, and progress indicators.

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

/// Health/status levels used throughout the UI
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HealthStatus {
    /// Everything is good
    Healthy,
    /// Warning - attention may be needed
    Warning,
    /// Critical - action required
    Critical,
    /// Informational - neutral status
    Info,
    /// Unknown or not applicable
    Unknown,
}

impl HealthStatus {
    /// Get the associated color for this status
    pub fn color(&self) -> Color {
        match self {
            HealthStatus::Healthy => Color::Green,
            HealthStatus::Warning => Color::Yellow,
            HealthStatus::Critical => Color::Red,
            HealthStatus::Info => Color::Cyan,
            HealthStatus::Unknown => Color::DarkGray,
        }
    }

    /// Get a style with just the foreground color
    pub fn style(&self) -> Style {
        Style::default().fg(self.color())
    }

    /// Get an emphasized style (bold)
    pub fn emphasized(&self) -> Style {
        self.style().add_modifier(Modifier::BOLD)
    }

    /// Get a dimmed style
    pub fn dimmed(&self) -> Style {
        Style::default().fg(match self {
            HealthStatus::Healthy => Color::DarkGray,
            HealthStatus::Warning => Color::DarkGray,
            HealthStatus::Critical => Color::DarkGray,
            HealthStatus::Info => Color::DarkGray,
            HealthStatus::Unknown => Color::DarkGray,
        })
    }
}

/// Helper for consistent status color coding
pub struct StatusColor;

impl StatusColor {
    /// Color for healthy/good/success states
    pub fn healthy() -> Color {
        Color::Green
    }

    /// Color for warning/attention states
    pub fn warning() -> Color {
        Color::Yellow
    }

    /// Color for critical/error states
    pub fn critical() -> Color {
        Color::Red
    }

    /// Color for informational states
    pub fn info() -> Color {
        Color::Cyan
    }

    /// Color for disabled/unknown/inactive states
    pub fn inactive() -> Color {
        Color::DarkGray
    }

    /// Color for best/optimal options (e.g., highest quality)
    pub fn best() -> Color {
        Color::Green
    }

    /// Color for worst/suboptimal options (e.g., lowest quality)
    pub fn worst() -> Color {
        Color::Red
    }

    /// Get color for a numeric value compared to best
    /// Returns green for best, red for worst, yellow for middle
    pub fn quality_color(value: u32, best: u32, worst: u32) -> Color {
        if value >= best {
            Color::Green
        } else if value <= worst {
            Color::Red
        } else {
            Color::Yellow
        }
    }

    /// Get color for a ratio (0.0 to 1.0)
    /// 0.0 = critical, 0.5 = warning, 1.0 = healthy
    pub fn ratio_color(ratio: f64) -> Color {
        if ratio >= 0.9 {
            Color::Green
        } else if ratio >= 0.5 {
            Color::Yellow
        } else {
            Color::Red
        }
    }
}

/// A status indicator widget for displaying health/progress
#[derive(Clone)]
pub struct StatusIndicator {
    label: String,
    value: String,
    status: HealthStatus,
    show_icon: bool,
}

impl StatusIndicator {
    pub fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            status: HealthStatus::Info,
            show_icon: true,
        }
    }

    pub fn status(mut self, status: HealthStatus) -> Self {
        self.status = status;
        self
    }

    pub fn healthy(mut self) -> Self {
        self.status = HealthStatus::Healthy;
        self
    }

    pub fn warning(mut self) -> Self {
        self.status = HealthStatus::Warning;
        self
    }

    pub fn critical(mut self) -> Self {
        self.status = HealthStatus::Critical;
        self
    }

    pub fn hide_icon(mut self) -> Self {
        self.show_icon = false;
        self
    }

    /// Get the icon for the current status
    pub fn icon(&self) -> &'static str {
        match self.status {
            HealthStatus::Healthy => "[OK]",
            HealthStatus::Warning => "[!]",
            HealthStatus::Critical => "[X]",
            HealthStatus::Info => "[i]",
            HealthStatus::Unknown => "[?]",
        }
    }

    /// Render as a Line
    pub fn render_line(&self) -> Line<'static> {
        let mut spans = Vec::new();

        if self.show_icon {
            spans.push(Span::styled(
                format!("{} ", self.icon()),
                self.status.style(),
            ));
        }

        spans.push(Span::raw(format!("{}: ", self.label.clone())));
        spans.push(Span::styled(self.value.clone(), self.status.style()));

        Line::from(spans)
    }

    /// Render as a simple styled span
    pub fn render_span(&self) -> Span<'static> {
        Span::styled(
            format!("{}: {}", self.label, self.value),
            self.status.style(),
        )
    }
}

/// Builder for status summary displays (like corpus health, deployment stats)
pub struct StatusSummary {
    indicators: Vec<StatusIndicator>,
    title: Option<String>,
}

impl Default for StatusSummary {
    fn default() -> Self {
        Self::new()
    }
}

impl StatusSummary {
    pub fn new() -> Self {
        Self {
            indicators: Vec::new(),
            title: None,
        }
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn indicator(mut self, indicator: StatusIndicator) -> Self {
        self.indicators.push(indicator);
        self
    }

    pub fn add(&mut self, indicator: StatusIndicator) {
        self.indicators.push(indicator);
    }

    /// Render as multiple lines
    pub fn render_lines(&self) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        if let Some(ref title) = self.title {
            lines.push(Line::from(Span::styled(
                title.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));
        }

        for indicator in &self.indicators {
            lines.push(indicator.render_line());
        }

        lines
    }

    /// Get overall status (worst of all indicators)
    pub fn overall_status(&self) -> HealthStatus {
        self.indicators
            .iter()
            .map(|i| i.status)
            .max_by_key(|s| match s {
                HealthStatus::Critical => 3,
                HealthStatus::Warning => 2,
                HealthStatus::Unknown => 1,
                HealthStatus::Info => 0,
                HealthStatus::Healthy => 0,
            })
            .unwrap_or(HealthStatus::Unknown)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health_status_colors() {
        assert_eq!(HealthStatus::Healthy.color(), Color::Green);
        assert_eq!(HealthStatus::Warning.color(), Color::Yellow);
        assert_eq!(HealthStatus::Critical.color(), Color::Red);
    }

    #[test]
    fn test_quality_color() {
        assert_eq!(StatusColor::quality_color(320, 320, 128), Color::Green);
        assert_eq!(StatusColor::quality_color(128, 320, 128), Color::Red);
        assert_eq!(StatusColor::quality_color(256, 320, 128), Color::Yellow);
    }

    #[test]
    fn test_status_summary() {
        let summary = StatusSummary::new()
            .indicator(StatusIndicator::new("Tracks", "1000").healthy())
            .indicator(StatusIndicator::new("Warnings", "5").warning());

        assert_eq!(summary.overall_status(), HealthStatus::Warning);
    }
}
