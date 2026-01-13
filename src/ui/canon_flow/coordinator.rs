//! Canon flow coordinator - handles flow lifecycle and transitions.
//!
//! This module contains the flow coordination logic that was previously
//! in `ui/mod.rs`. App methods delegate to these functions.

use crate::config;
use crate::corpus::db::PendingChange;
use crate::corpus::health::{get_genre_collisions, TagCloud};
use crate::ops::operation::{OperationProgress, OperationType, ProgressReporter};
use crate::ui::helpers::open_database;
use crate::ui::{App, CanonCommitModalState, UiMode};

use super::{
    ArtistBucket, ArtistVariant, CanonSession, ClusterViewAction, ClusterViewState,
    ReviewAction, ReviewState,
};

// ============================================================================
// Flow Start Functions
// ============================================================================

/// Start the artist canonicalization flow.
///
/// Loads artist buckets with variants from the database and initializes
/// the cluster view.
pub fn start(app: &mut App) {
    let db = match open_database(&mut app.status_message) {
        Some(db) => db,
        None => return,
    };

    // Get artist buckets with variants
    match db.get_artist_canonicalization_buckets() {
        Ok(bucket_data) if !bucket_data.is_empty() => {
            // Convert to ArtistBucket structs
            let buckets: Vec<ArtistBucket> = bucket_data
                .into_iter()
                .map(|(normalized_key, variants)| ArtistBucket {
                    normalized_key,
                    variants: variants
                        .into_iter()
                        .map(|(name, track_count)| ArtistVariant {
                            name,
                            track_count,
                            selected_for_squash: false,
                        })
                        .collect(),
                })
                .collect();

            let session = CanonSession::new(uuid::Uuid::new_v4().to_string(), buckets);

            app.canon_cluster_view = Some(ClusterViewState::new(session));
            app.mode = UiMode::CanonClusterView;
        }
        Ok(_) => {
            app.status_message =
                Some("No artist name variants found - corpus is clean!".to_string());
        }
        Err(e) => {
            app.status_message = Some(format!("Error loading artist buckets: {}", e));
        }
    }
}

/// Start the genre canonicalization flow.
///
/// Uses TagCloud to detect genre collisions and reports findings.
/// (Full UI coming later - currently just logs and reports)
pub fn start_genre(app: &mut App) {
    let db = match open_database(&mut app.status_message) {
        Some(db) => db,
        None => return,
    };

    // Build tag cloud
    let cloud = match TagCloud::build(&db) {
        Ok(c) => c,
        Err(e) => {
            app.status_message = Some(format!("TagCloud build error: {}", e));
            return;
        }
    };

    // Get genre collisions
    let collisions = get_genre_collisions(&cloud);

    if collisions.is_empty() {
        app.status_message = Some("No genre variants found - genres are clean!".to_string());
        return;
    }

    // For now, just report what was found
    let total_variants: usize = collisions.iter().map(|c| c.variants.len()).sum();
    app.status_message = Some(format!(
        "Found {} genre collisions with {} total variants. Full UI coming soon.",
        collisions.len(),
        total_variants
    ));

    // Log details
    for collision in &collisions {
        let _ = config::log_message(&format!(
            "Genre collision: {} -> {:?} (canonical: {})",
            collision.normalized_key, collision.variants, collision.canonical
        ));
    }
}

// ============================================================================
// Action Handlers
// ============================================================================

/// Handle cluster view actions.
pub fn handle_cluster_action(app: &mut App, action: ClusterViewAction) {
    match action {
        ClusterViewAction::None => {}
        ClusterViewAction::Continue => {}
        ClusterViewAction::SessionComplete => {
            transition_to_review(app);
        }
        ClusterViewAction::ShowSessionReview => {
            transition_to_review(app);
        }
        ClusterViewAction::StatusMessage(msg) => {
            app.status_message = Some(msg);
        }
    }
}

/// Transition from cluster view to session review.
fn transition_to_review(app: &mut App) {
    if let Some(cluster_view) = app.canon_cluster_view.take() {
        let session = cluster_view.into_session();

        if session.decisions.is_empty() {
            app.status_message = Some("No decisions made".to_string());
            app.mode = UiMode::MainMenu;
        } else {
            app.canon_session_review = Some(ReviewState::new(session));
            app.mode = UiMode::CanonSessionReview;
        }
    }
}

/// Handle session review actions.
pub fn handle_review_action(app: &mut App, action: ReviewAction) {
    match action {
        ReviewAction::None => {}
        ReviewAction::Continue => {}
        ReviewAction::Commit => {
            commit_changes(app);
        }
        ReviewAction::Cancel => {
            app.canon_session_review = None;
            app.mode = UiMode::MainMenu;
            app.status_message = Some("Artist canonicalization cancelled".to_string());
        }
        ReviewAction::BackToClusterView => {
            // Return to cluster view to add more decisions
            if let Some(review) = app.canon_session_review.take() {
                let session = review.into_session();
                app.canon_cluster_view = Some(ClusterViewState::new(session));
                app.mode = UiMode::CanonClusterView;
            }
        }
    }
}

// ============================================================================
// Commit and Tag Flush
// ============================================================================

/// Commit canon changes to the database.
///
/// Updates artist tags in the database, records tag mismatches,
/// and starts a background tag flush to update files on disk.
pub fn commit_changes(app: &mut App) {
    let _ = config::log_message("=== CANON REVIEW: COMMIT REQUESTED ===");

    // Get all pending changes from decisions
    let all_changes: Vec<PendingChange> = if let Some(ref review) = app.canon_session_review {
        review
            .session()
            .decisions
            .iter()
            .flat_map(|d| d.pending_changes.clone())
            .collect()
    } else {
        vec![]
    };

    let _ = config::log_message(&format!(
        "Total pending tag changes to execute: {}",
        all_changes.len()
    ));

    if all_changes.is_empty() {
        app.canon_session_review = None;
        app.mode = UiMode::MainMenu;
        app.status_message = Some("No changes to commit".to_string());
        return;
    }

    // Open database
    let db = match open_database(&mut app.status_message) {
        Some(db) => db,
        None => {
            app.canon_session_review = None;
            app.mode = UiMode::MainMenu;
            return;
        }
    };

    // Update database artist fields directly
    let mut succeeded = 0;
    let mut failed = 0;

    for change in &all_changes {
        if let Some(ref metadata_json) = change.metadata_changes {
            if let Ok(metadata) = serde_json::from_str::<serde_json::Value>(metadata_json) {
                let new_artist = metadata
                    .get("new_artist")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let old_artist = metadata.get("old_artist").and_then(|v| v.as_str());

                // Get track ID by path
                if let Ok(Some(track)) = db.get_track_by_path(&change.source_path) {
                    if let Some(track_id) = track.id {
                        match db.update_artist_for_tracks(&[track_id], new_artist) {
                            Ok(_) => {
                                succeeded += 1;
                                // Record tag mismatch: disk still has old value, DB now has new value
                                if let Err(e) = db.record_tag_mismatch(
                                    track_id,
                                    "artist",
                                    Some(new_artist), // db_value
                                    old_artist,       // disk_value
                                ) {
                                    let _ = config::log_message(&format!(
                                        "Failed to record tag mismatch for {}: {}",
                                        change.source_path, e
                                    ));
                                }
                            }
                            Err(e) => {
                                let _ = config::log_message(&format!(
                                    "Failed to update artist for {}: {}",
                                    change.source_path, e
                                ));
                                failed += 1;
                            }
                        }
                    }
                }
            }
        }
    }

    let _ = config::log_message(&format!(
        "Canon commit complete: {} succeeded, {} failed",
        succeeded, failed
    ));

    // Compute stale deployment count
    let stale_count = match crate::ops::deploy::compute_full_deployment_status(&app.config, &db) {
        Ok(statuses) => statuses.iter().map(|s| s.stale.len()).sum(),
        Err(_) => 0,
    };

    let _ = config::log_message(&format!(
        "Stale deployments after commit: {}",
        stale_count
    ));

    // Show commit modal with options
    app.canon_session_review = None;
    app.canon_commit_modal_state = Some(CanonCommitModalState {
        selected_option: 1, // Default to deployment review
        tracks_updated: succeeded,
        stale_deployments: stale_count,
    });
    app.mode = UiMode::CanonCommitModal;

    // Start background tag-flush operation
    start_tag_flush(app, all_changes);
}

/// Start background tag flush to write artist tags to disk.
pub fn start_tag_flush(app: &mut App, changes: Vec<PendingChange>) {
    let change_count = changes.len();

    // Add operation to manager
    let (id, tx, cancel_flag) = app.operations.add(OperationType::ExecutingChanges {
        session_id: "canon_tag_flush".to_string(),
        change_count,
    });

    let _ = config::log_message(&format!(
        "Starting background tag flush for {} tracks [{}]",
        change_count, id
    ));

    // Spawn background thread to write tags to disk
    std::thread::spawn(move || {
        use std::time::Instant;

        let start = Instant::now();
        let mut reporter = ProgressReporter::new(tx.clone(), cancel_flag.clone());

        let mut progress = OperationProgress::new(changes.len());
        reporter.force_update(progress.clone());

        let mut succeeded = 0;
        let mut failed = 0;
        let mut errors = Vec::new();
        let mut flushed_paths = Vec::new();

        for change in &changes {
            if reporter.is_cancelled() {
                reporter.cancelled();
                return;
            }

            progress.current_item = Some(change.source_path.clone());
            reporter.update(progress.clone());

            // Parse metadata to get new artist
            if let Some(ref metadata_json) = change.metadata_changes {
                if let Ok(metadata) = serde_json::from_str::<serde_json::Value>(metadata_json) {
                    let new_artist = metadata
                        .get("new_artist")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");

                    // Write tag to file using lofty
                    match crate::corpus::metadata::write_artist_tag(&change.source_path, new_artist)
                    {
                        Ok(_) => {
                            succeeded += 1;
                            flushed_paths.push(change.source_path.clone());
                        }
                        Err(e) => {
                            errors.push(format!("{}: {}", change.source_path, e));
                            failed += 1;
                        }
                    }
                }
            }

            progress.completed_items += 1;
            reporter.update(progress.clone());
        }

        // Store flushed paths in result data for mismatch cleanup
        let result = crate::ops::operation::OperationResult {
            succeeded,
            skipped: 0,
            failed,
            duration: start.elapsed(),
            bytes_processed: None,
            errors,
            data: Some(crate::ops::operation::ResultData::TagFlush { flushed_paths }),
        };
        reporter.complete(result);
    });
}
