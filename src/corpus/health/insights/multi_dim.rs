//! Multi-Dimensional Insight Computations
//!
//! These insights correlate multiple signal types and require background
//! computation. They're spawned as tasks with progress tracking.
//!
//! ## Quality Duplicates
//!
//! Correlates FingerprintDuplicate + QualityVariant signals to identify
//! duplicate groups where a quality winner can be determined automatically.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Instant;

use crate::corpus::db::types::HealthIssueType;
use crate::corpus::db::Database;
use crate::flows::background::{ProgressSender, TaskMessage, TaskProgress, TaskResult};

use super::Insight;

/// Types of multi-dimensional insights that require background computation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MultiDimInsightType {
    /// Correlate FingerprintDuplicate + QualityVariant signals
    QualityDuplicates,
    // Future:
    // TagConflictDuplicates,
    // CrossAlbumArtistConsolidation,
}

impl MultiDimInsightType {
    /// Display name for progress UI
    pub fn label(&self) -> &'static str {
        match self {
            MultiDimInsightType::QualityDuplicates => "Quality Duplicates",
        }
    }
}

/// Handle for a spawned multi-dim insight computation.
pub struct InsightHandle {
    pub receiver: mpsc::Receiver<TaskMessage>,
    pub cancel_flag: Arc<AtomicBool>,
}

impl InsightHandle {
    /// Request cancellation.
    pub fn cancel(&self) {
        self.cancel_flag.store(true, Ordering::SeqCst);
    }

    /// Non-blocking poll for the next message.
    pub fn try_recv(&self) -> Option<TaskMessage> {
        self.receiver.try_recv().ok()
    }
}

/// Spawn a background computation for a multi-dimensional insight.
///
/// Returns an `InsightHandle` for progress tracking and cancellation.
///
/// # Arguments
///
/// * `db_path` - Path to database file (needed for thread-local connection)
/// * `insight_type` - Which multi-dim insight to compute
///
/// # Returns
///
/// `InsightHandle` with receiver for progress messages.
pub fn spawn_multi_dim_insight(db_path: &str, insight_type: MultiDimInsightType) -> InsightHandle {
    let db_path = db_path.to_string();

    // Create handle components
    let (tx, rx) = mpsc::channel();
    let cancel_flag = Arc::new(AtomicBool::new(false));
    let cancel_flag_clone = cancel_flag.clone();

    // Spawn computation thread
    thread::spawn(move || {
        match insight_type {
            MultiDimInsightType::QualityDuplicates => {
                compute_quality_duplicates(&db_path, tx, cancel_flag_clone);
            }
        }
    });

    InsightHandle {
        receiver: rx,
        cancel_flag,
    }
}

/// Compute quality duplicates by correlating fingerprint dupes with quality info.
///
/// This computation:
/// 1. Loads all FingerprintDuplicate issues
/// 2. Loads all QualityVariant issues
/// 3. For each dupe group, checks if quality comparison is possible
/// 4. Counts auto-resolvable (clear winner) vs needs-decision groups
fn compute_quality_duplicates(
    db_path: &str,
    tx: mpsc::Sender<TaskMessage>,
    cancel_flag: Arc<AtomicBool>,
) {
    let start = Instant::now();
    let mut reporter = ProgressSender::new(tx.clone(), cancel_flag.clone());

    // Open thread-local database connection
    let db = match Database::open(Path::new(&db_path)) {
        Ok(db) => db,
        Err(e) => {
            reporter.error(format!("Failed to open database: {}", e));
            return;
        }
    };

    // Step 1: Load fingerprint duplicate issues
    let fp_dupes = match db.get_unresolved_health_issues(Some(HealthIssueType::FingerprintDuplicate))
    {
        Ok(issues) => issues,
        Err(e) => {
            reporter.error(format!("Failed to load fingerprint duplicates: {}", e));
            return;
        }
    };

    let total_groups = fp_dupes.len();
    if total_groups == 0 {
        // No duplicates - complete immediately
        let _ = tx.send(TaskMessage::Complete(TaskResult {
            succeeded: 0,
            skipped: 0,
            failed: 0,
            duration: start.elapsed(),
            bytes_processed: None,
            errors: Vec::new(),
        }));
        return;
    }

    // Step 2: Load quality variant issues and build fingerprint -> quality map
    let quality_issues =
        db.get_unresolved_health_issues(Some(HealthIssueType::QualityVariant));
    let quality_fps: std::collections::HashSet<String> = quality_issues
        .unwrap_or_default()
        .into_iter()
        .map(|i| i.issue_key)
        .collect();

    // Step 3: Process each duplicate group
    let mut auto_resolvable = 0;
    let mut needs_decision = 0;
    let mut total_tracks = 0;
    let mut errors = Vec::new();

    let mut progress = TaskProgress::new(total_groups);
    reporter.force_update(progress.clone());

    for (i, dupe) in fp_dupes.iter().enumerate() {
        // Check for cancellation
        if cancel_flag.load(Ordering::Relaxed) {
            reporter.error("Cancelled".to_string());
            return;
        }

        // Get tracks for this duplicate group
        let tracks = match db.get_health_issue_tracks(dupe.id.unwrap_or(0)) {
            Ok(tracks) => tracks,
            Err(e) => {
                errors.push(format!("Failed to get tracks for issue {}: {}", dupe.id.unwrap_or(0), e));
                continue;
            }
        };

        total_tracks += tracks.len();

        // Check if this fingerprint has quality variant info
        if quality_fps.contains(&dupe.issue_key) {
            // Has quality info - check if there's a clear winner
            // For now, assume quality variants mean auto-resolvable
            // (in reality, we'd compare bitrates/formats)
            if has_clear_quality_winner(&tracks) {
                auto_resolvable += 1;
            } else {
                needs_decision += 1;
            }
        } else {
            // No quality info - needs manual decision
            needs_decision += 1;
        }

        // Update progress periodically
        progress.completed = i + 1;
        progress.current_item = Some(format!("Group {}/{}", i + 1, total_groups));
        reporter.update(progress.clone());
    }

    // Send final progress
    progress.completed = total_groups;
    progress.current_item = None;
    reporter.force_update(progress);

    // Build the insight result (available via result_to_insight)
    let _insight = Insight::QualityDuplicates {
        dupe_groups: total_groups,
        total_tracks,
        auto_resolvable,
    };

    // Complete with result
    let result = TaskResult {
        succeeded: auto_resolvable,
        skipped: needs_decision,
        failed: errors.len(),
        duration: start.elapsed(),
        bytes_processed: None,
        errors,
    };

    reporter.complete(result);
}

/// Determine if there's a clear quality winner among tracks.
///
/// A clear winner exists when:
/// - One track has significantly higher bitrate than others
/// - Or one track is lossless while others are lossy
fn has_clear_quality_winner(tracks: &[(crate::corpus::db::types::Track, crate::corpus::db::types::TrackRole)]) -> bool {
    if tracks.len() < 2 {
        return false;
    }

    // Extract bitrates and file types
    let mut qualities: Vec<(i32, bool)> = tracks
        .iter()
        .filter_map(|(track, _)| {
            let bitrate = track.bitrate_kbps.unwrap_or(0);
            let is_lossless = matches!(track.file_type.as_str(), "flac" | "alac" | "wav" | "aiff");
            Some((bitrate, is_lossless))
        })
        .collect();

    if qualities.is_empty() {
        return false;
    }

    // Check for lossless vs lossy (clear winner)
    let has_lossless = qualities.iter().any(|(_, is_ll)| *is_ll);
    let has_lossy = qualities.iter().any(|(_, is_ll)| !*is_ll);
    if has_lossless && has_lossy {
        return true; // Lossless wins over lossy
    }

    // Check for significant bitrate difference (> 50% higher than lowest)
    qualities.sort_by_key(|(br, _)| *br);
    let lowest = qualities.first().map(|(br, _)| *br).unwrap_or(0);
    let highest = qualities.last().map(|(br, _)| *br).unwrap_or(0);

    if lowest > 0 && highest > lowest {
        let ratio = highest as f64 / lowest as f64;
        return ratio > 1.5; // 50% higher bitrate is a clear winner
    }

    false
}

/// Convert a TaskResult back to an Insight.
///
/// This is used by the UI to extract the computed insight from
/// the task completion message.
pub fn result_to_insight(
    insight_type: MultiDimInsightType,
    result: &TaskResult,
    total_groups: usize,
) -> Insight {
    match insight_type {
        MultiDimInsightType::QualityDuplicates => Insight::QualityDuplicates {
            dupe_groups: total_groups,
            total_tracks: 0, // Not easily recoverable from result
            auto_resolvable: result.succeeded,
        },
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::corpus::db::types::{Track, TrackRole};

    fn mock_track(bitrate: i32, file_type: &str) -> Track {
        Track {
            id: Some(1),
            path: "/test/track.mp3".to_string(),
            source: "corpus".to_string(),
            inode: 12345,
            file_size: 1000000,
            file_type: file_type.to_string(),
            artist: Some("Artist".to_string()),
            album: Some("Album".to_string()),
            album_artist: None,
            title: Some("Title".to_string()),
            track_number: Some(1),
            genre: None,
            duration_ms: Some(180000),
            bitrate_kbps: Some(bitrate),
            sample_rate: Some(44100),
            fingerprint: None,
            isrc: None,
        }
    }

    #[test]
    fn test_has_clear_winner_lossless_vs_lossy() {
        let tracks = vec![
            (mock_track(320, "mp3"), TrackRole::Member),
            (mock_track(1411, "flac"), TrackRole::Member),
        ];
        assert!(has_clear_quality_winner(&tracks));
    }

    #[test]
    fn test_has_clear_winner_significant_bitrate_diff() {
        let tracks = vec![
            (mock_track(128, "mp3"), TrackRole::Member),
            (mock_track(320, "mp3"), TrackRole::Member),
        ];
        assert!(has_clear_quality_winner(&tracks));
    }

    #[test]
    fn test_no_clear_winner_similar_bitrates() {
        let tracks = vec![
            (mock_track(256, "mp3"), TrackRole::Member),
            (mock_track(320, "mp3"), TrackRole::Member),
        ];
        // 320/256 = 1.25, less than 1.5 threshold
        assert!(!has_clear_quality_winner(&tracks));
    }

    #[test]
    fn test_no_clear_winner_single_track() {
        let tracks = vec![(mock_track(320, "mp3"), TrackRole::Member)];
        assert!(!has_clear_quality_winner(&tracks));
    }
}
