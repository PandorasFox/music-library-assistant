//! Report Generation Module
//!
//! Provides the ReportRenderer trait and infrastructure for generating
//! corpus analysis reports.
//!
//! ## Architecture
//!
//! Reports implement the `ReportRenderer` trait which provides:
//! - `render_text()` - Full text report for file output
//! - `render_summary()` - Lightweight summary for UI display
//!
//! ## Report Types
//!
//! Currently available via `crate::flows::reports`:
//! - Legacy library matching report
//! - Fingerprint duplicates report
//! - Metadata duplicates report
//! - Health issues report
//! - Known variants report
//! - Deployment report
//!
//! ## Future Migration
//!
//! Report implementations will be migrated from `ops/reports.rs` to
//! individual modules here as they're updated to use the ReportRenderer trait.

mod renderer;

// Re-export report infrastructure (for use by future report implementations)
#[allow(unused_imports)]
pub use renderer::{write_report_to_file, ReportRenderer, ReportSummary};
