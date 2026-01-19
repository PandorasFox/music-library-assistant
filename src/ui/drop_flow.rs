//! Drop missing files flow.
//!
//! Handles the UI for confirming and executing removal of missing
//! files from the corpus index.

use crossterm::event::{KeyCode, KeyEvent};

use std::path::Path;

use crate::corpus::db::{Database, Track};
use crate::flows::{changes, DecisionType, PendingDecision};

/// State for drop missing confirmation dialog.
#[derive(Debug, Clone)]
pub struct DropMissingState {
    pub missing_tracks: Vec<Track>,
    pub list_offset: usize,
    pub selected_option: usize, // 0 = Cancel, 1 = Drop
}

/// Actions returned from drop missing key handling.
#[derive(Debug)]
pub enum DropMissingAction {
    None,
    Execute,
    Cancel,
}

impl DropMissingState {
    pub fn new(missing_tracks: Vec<Track>) -> Self {
        Self {
            missing_tracks,
            list_offset: 0,
            selected_option: 0, // Default to Cancel
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> DropMissingAction {
        let list_len = self.missing_tracks.len();
        let max_visible = 20; // Number of items visible in the list

        match key.code {
            KeyCode::Up => {
                if self.list_offset > 0 {
                    self.list_offset -= 1;
                }
                DropMissingAction::None
            }
            KeyCode::Down => {
                if self.list_offset + max_visible < list_len {
                    self.list_offset += 1;
                }
                DropMissingAction::None
            }
            KeyCode::Left | KeyCode::Right => {
                // Toggle between Cancel (0) and Drop (1)
                self.selected_option = 1 - self.selected_option;
                DropMissingAction::None
            }
            KeyCode::Enter => {
                if self.selected_option == 1 {
                    DropMissingAction::Execute
                } else {
                    DropMissingAction::Cancel
                }
            }
            KeyCode::Esc => DropMissingAction::Cancel,
            _ => DropMissingAction::None,
        }
    }
}

/// Result of executing the drop missing operation.
pub struct DropMissingResult {
    pub dropped_count: usize,
    pub failed_count: usize,
    pub orphans_cleaned: usize,
    pub log_path: Option<String>,
    pub errors: Vec<String>,
}

/// Find tracks in the index whose files no longer exist on disk.
pub fn find_missing_tracks(db: &Database) -> Result<Vec<Track>, String> {
    // Get all corpus tracks
    let tracks = db
        .get_all_tracks(Some("corpus"))
        .map_err(|e| format!("Database error: {}", e))?;

    // Filter to those missing from disk
    let missing: Vec<Track> = tracks
        .into_iter()
        .filter(|track| !Path::new(&track.path).exists())
        .collect();

    Ok(missing)
}

/// Execute the drop missing operation.
///
/// NOTE: This uses vestigial execute_decisions which always fails.
/// Needs refactoring to use daemon's transaction API (queue_mutations).
pub fn execute_drop_missing(db: &Database, missing_tracks: &[Track]) -> Result<DropMissingResult, String> {
    use crate::config;
    use std::fs::File;
    use std::io::Write;

    if missing_tracks.is_empty() {
        return Ok(DropMissingResult {
            dropped_count: 0,
            failed_count: 0,
            orphans_cleaned: 0,
            log_path: None,
            errors: vec![],
        });
    }

    // Get reports directory for log file
    let reports_dir = config::get_data_dir()
        .map_err(|e| format!("Failed to get data dir: {}", e))?
        .join("reports");

    // Create reports directory if needed
    std::fs::create_dir_all(&reports_dir)
        .map_err(|e| format!("Failed to create reports dir: {}", e))?;

    // Write log file with dropped track metadata
    let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let log_path = reports_dir.join(format!("dropped_tracks_{}.log", timestamp));
    let log_created = (|| -> Result<(), std::io::Error> {
        let mut log_file = File::create(&log_path)?;
        writeln!(log_file, "# Tracks dropped from corpus index")?;
        writeln!(log_file, "# Timestamp: {}", chrono::Local::now())?;
        writeln!(log_file, "# Count: {}", missing_tracks.len())?;
        writeln!(log_file, "#")?;
        for track in missing_tracks {
            writeln!(log_file, "Path: {}", track.path)?;
            writeln!(log_file, "  ID: {:?}", track.id)?;
            writeln!(log_file)?;
        }
        Ok(())
    })()
    .is_ok();

    // Generate DropIndex decisions for each missing track
    let pending_decisions: Vec<PendingDecision> = missing_tracks
        .iter()
        .map(|track| PendingDecision {
            decision_type: DecisionType::DropIndex,
            source_path: track.path.clone(),
            target_path: None,
            metadata: None,
        })
        .collect();

    // Execute the decisions (NOTE: vestigial, will always fail)
    let report = changes::execute_decisions(db, &pending_decisions, false)
        .map_err(|e| format!("Drop error: {}", e))?;

    // Clean up orphaned scan_state entries
    let orphan_cleanup = db.cleanup_missing_scan_state_entries("corpus").unwrap_or(0);

    Ok(DropMissingResult {
        dropped_count: report.succeeded,
        failed_count: report.failed,
        orphans_cleaned: orphan_cleanup,
        log_path: if log_created {
            Some(log_path.display().to_string())
        } else {
            None
        },
        errors: report.errors,
    })
}
