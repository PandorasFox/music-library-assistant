//! Drop missing files flow.
//!
//! Handles the UI for confirming and executing removal of missing
//! files from the corpus index.

use crossterm::event::{KeyCode, KeyEvent};

use crate::config;
use crate::corpus::db::{ChangeStatus, ChangeType, Database, PendingChange, Track};
use crate::ops::{changes, scanner};

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

/// Find missing tracks in the corpus.
pub fn find_missing_tracks() -> Result<Vec<Track>, String> {
    let db_path = config::get_db_path().map_err(|e| format!("Config error: {}", e))?;
    let db = Database::open(&db_path).map_err(|e| format!("Database error: {}", e))?;

    scanner::find_missing_tracks(&db, "corpus").map_err(|e| format!("Detection error: {}", e))
}

/// Execute the drop missing operation.
pub fn execute_drop_missing(missing_tracks: &[Track]) -> Result<DropMissingResult, String> {
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
            if let Some(ref artist) = track.artist {
                writeln!(log_file, "  Artist: {}", artist)?;
            }
            if let Some(ref title) = track.title {
                writeln!(log_file, "  Title: {}", title)?;
            }
            if let Some(ref album) = track.album {
                writeln!(log_file, "  Album: {}", album)?;
            }
            writeln!(log_file)?;
        }
        Ok(())
    })()
    .is_ok();

    // Open database
    let db_path = config::get_db_path().map_err(|e| format!("Config error: {}", e))?;
    let db = Database::open(&db_path).map_err(|e| format!("Database error: {}", e))?;

    // Generate DropIndex changes for each missing track
    let session_id = uuid::Uuid::new_v4().to_string();
    let pending_changes: Vec<PendingChange> = missing_tracks
        .iter()
        .map(|track| PendingChange {
            id: None,
            session_id: session_id.clone(),
            change_type: ChangeType::DropIndex,
            source_path: track.path.clone(),
            target_path: None,
            metadata_changes: None,
            created_at: None,
            status: ChangeStatus::Pending,
        })
        .collect();

    // Execute the changes
    let report = changes::execute_changes(&db, &pending_changes, false)
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
