//! Report Renderer Trait
//!
//! Defines the interface for report generation with both full text output
//! and lightweight summaries for UI display.

use anyhow::Result;

use crate::db::Database;

/// Summary data returned from a report for UI display.
///
/// This provides a lightweight representation of report results
/// that can be displayed in info panels without the overhead
/// of generating the full text report.
#[derive(Debug, Clone)]
pub struct ReportSummary {
    /// Report title for display
    pub title: String,
    /// Number of items found/processed
    pub item_count: usize,
    /// One-line summary (e.g., "Found 42 duplicate groups")
    pub brief: String,
    /// Multi-line details for info panel display
    pub details: Vec<String>,
}

impl ReportSummary {
    /// Create a new report summary.
    pub fn new(title: &str, item_count: usize, brief: &str) -> Self {
        Self {
            title: title.to_string(),
            item_count,
            brief: brief.to_string(),
            details: Vec::new(),
        }
    }

    /// Add a detail line to the summary.
    pub fn add_detail(&mut self, detail: &str) {
        self.details.push(detail.to_string());
    }

    /// Builder pattern for adding details.
    pub fn with_details(mut self, details: Vec<String>) -> Self {
        self.details = details;
        self
    }
}

/// Trait for report generation.
///
/// Each report type implements this trait to provide both full text
/// output (written to file) and lightweight summaries (for UI display).
///
/// ## Usage
///
/// ```ignore
/// struct DuplicateReport;
///
/// impl ReportRenderer for DuplicateReport {
///     fn report_type(&self) -> &'static str {
///         "duplicates"
///     }
///
///     fn render_text(&self, db: &Database) -> Result<String> {
///         // Generate full report text
///         Ok("...".to_string())
///     }
///
///     fn render_summary(&self, db: &Database) -> Result<ReportSummary> {
///         // Generate lightweight summary
///         Ok(ReportSummary::new("Duplicates", 42, "Found 42 duplicate groups"))
///     }
/// }
/// ```
pub trait ReportRenderer {
    /// Generate full text report content.
    ///
    /// This is called when writing the report to a file.
    fn render_text(&self, db: &Database) -> Result<String>;

    /// Generate summary for UI display.
    ///
    /// This should be quick and avoid expensive operations.
    /// The summary is displayed in info panels and status areas.
    fn render_summary(&self, db: &Database) -> Result<ReportSummary>;

    /// Report type identifier.
    ///
    /// Used for logging, file naming, and UI display.
    fn report_type(&self) -> &'static str;
}

/// Write a report to file using the renderer.
///
/// This is a convenience function that renders the report and writes
/// it to the specified path.
pub fn write_report_to_file<R: ReportRenderer>(
    renderer: &R,
    db: &Database,
    output_path: &std::path::Path,
) -> Result<ReportSummary> {
    use std::fs::File;
    use std::io::Write;

    let content = renderer.render_text(db)?;
    let summary = renderer.render_summary(db)?;

    let mut file = File::create(output_path)?;
    file.write_all(content.as_bytes())?;

    Ok(summary)
}
