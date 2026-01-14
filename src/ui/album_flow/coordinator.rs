//! Album flow coordinator - handles flow lifecycle and transitions.
//!
//! This module contains the flow coordination logic that was previously
//! in `ui/mod.rs`. App methods delegate to these functions.

use crate::corpus::health::album_normalization::normalize_album;
use crate::ui::helpers::open_database;
use crate::ui::{App, UiMode};

use super::{
    AlbumBucket, AlbumCanonSession, AlbumClusterAction, AlbumClusterState, AlbumReviewAction,
    AlbumReviewState, AlbumVariant,
};

// ============================================================================
// Flow Start
// ============================================================================

/// Start the album canonicalization flow.
///
/// Loads album buckets with variants from the database and initializes
/// the cluster view with EP/edition detection.
pub(crate) fn start(app: &mut App) {
    let db = match open_database(&mut app.status_message) {
        Some(db) => db,
        None => return,
    };

    // Get album buckets with variants
    match db.get_album_canonicalization_buckets() {
        Ok(bucket_data) if !bucket_data.is_empty() => {
            // Convert to AlbumBucket structs with EP/edition detection and enhanced info
            let buckets: Vec<AlbumBucket> = bucket_data
                .into_iter()
                .map(|(normalized_key, variants)| {
                    let album_variants: Vec<AlbumVariant> = variants
                        .into_iter()
                        .map(|(name, track_count)| {
                            let normalized = normalize_album(&name);
                            // Fetch additional details for this variant
                            let (artists, directories, file_types) = db
                                .get_album_variant_details(&name)
                                .unwrap_or_else(|_| (Vec::new(), Vec::new(), Vec::new()));
                            AlbumVariant {
                                name,
                                track_count,
                                normalized,
                                artists,
                                directories,
                                file_types,
                            }
                        })
                        .collect();

                    // Detect format and edition variants
                    let has_format_variants = {
                        let formats: std::collections::HashSet<_> = album_variants
                            .iter()
                            .map(|v| std::mem::discriminant(&v.normalized.format_type))
                            .collect();
                        formats.len() > 1
                    };

                    let has_edition_variants = {
                        let editions: std::collections::HashSet<_> = album_variants
                            .iter()
                            .map(|v| v.normalized.edition.as_ref().map(|e| e.to_lowercase()))
                            .collect();
                        editions.len() > 1
                    };

                    AlbumBucket {
                        normalized_key,
                        variants: album_variants,
                        has_format_variants,
                        has_edition_variants,
                    }
                })
                .collect();

            let session =
                AlbumCanonSession::new(uuid::Uuid::new_v4().to_string(), buckets);

            app.album_cluster_view = Some(AlbumClusterState::new(session));
            app.mode = UiMode::AlbumClusterView;
        }
        Ok(_) => {
            app.status_message =
                Some("No album name variants found - corpus is clean!".to_string());
        }
        Err(e) => {
            app.status_message = Some(format!("Error loading album buckets: {}", e));
        }
    }
}

// ============================================================================
// Action Handlers
// ============================================================================

/// Handle cluster view actions.
pub(crate) fn handle_cluster_action(app: &mut App, action: AlbumClusterAction) {
    match action {
        AlbumClusterAction::None => {}
        AlbumClusterAction::Continue => {}
        AlbumClusterAction::SessionComplete => {
            // Move to review
            if let Some(cluster_view) = app.album_cluster_view.take() {
                let session = cluster_view.into_session();
                if session.decisions.is_empty() {
                    app.status_message = Some("No album changes recorded".to_string());
                    app.mode = UiMode::MainMenu;
                } else {
                    app.album_review = Some(AlbumReviewState::new(session));
                    app.mode = UiMode::AlbumReview;
                }
            }
        }
        AlbumClusterAction::ShowSessionReview => {
            // Escape behavior: back if no changes, review if changes
            if let Some(cluster_view) = app.album_cluster_view.take() {
                let session = cluster_view.into_session();
                if session.decisions.is_empty() {
                    // No changes - go back to main menu
                    app.status_message = Some("No changes made".to_string());
                    app.mode = UiMode::MainMenu;
                } else {
                    // Has changes - go to review
                    app.album_review = Some(AlbumReviewState::new(session));
                    app.mode = UiMode::AlbumReview;
                }
            }
        }
        AlbumClusterAction::StatusMessage(msg) => {
            app.status_message = Some(msg);
        }
    }
}

/// Handle review actions.
pub(crate) fn handle_review_action(app: &mut App, action: AlbumReviewAction) {
    match action {
        AlbumReviewAction::None => {}
        AlbumReviewAction::Continue => {}
        AlbumReviewAction::Commit => {
            if let Some(review) = app.album_review.take() {
                let session = review.into_session();
                commit_decisions(app, session);
            }
        }
        AlbumReviewAction::Cancel => {
            app.album_review = None;
            app.status_message = Some("Album canonicalization cancelled".to_string());
            app.mode = UiMode::MainMenu;
        }
        AlbumReviewAction::BackToClusterView => {
            if let Some(review) = app.album_review.take() {
                let session = review.into_session();
                app.album_cluster_view = Some(AlbumClusterState::new(session));
                app.mode = UiMode::AlbumClusterView;
            }
        }
    }
}

// ============================================================================
// Commit
// ============================================================================

/// Commit album decisions to the database.
pub(crate) fn commit_decisions(app: &mut App, session: AlbumCanonSession) {
    let db = match open_database(&mut app.status_message) {
        Some(db) => db,
        None => {
            app.mode = UiMode::MainMenu;
            return;
        }
    };

    let mut total_updated = 0;
    let mut errors = Vec::new();

    // Process each decision
    for decision in &session.decisions {
        for variant_name in &decision.variants_to_rename {
            match db.get_track_ids_by_album(variant_name) {
                Ok(track_ids) => {
                    match db.update_album_for_tracks(&track_ids, &decision.canonical_name) {
                        Ok(count) => total_updated += count,
                        Err(e) => errors.push(format!("{}: {}", variant_name, e)),
                    }
                }
                Err(e) => errors.push(format!("{}: {}", variant_name, e)),
            }
        }
    }

    if errors.is_empty() {
        app.status_message = Some(format!(
            "Album canonicalization complete: {} tracks updated",
            total_updated
        ));
    } else {
        app.status_message = Some(format!(
            "Album canonicalization completed with {} errors: {}",
            errors.len(),
            errors.join(", ")
        ));
    }

    app.album_review = None;
    app.mode = UiMode::MainMenu;
}
