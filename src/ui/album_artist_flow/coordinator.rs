//! Album artist flow coordinator - handles single-phase flow lifecycle.
//!
//! This module contains the flow coordination logic that was previously
//! in `ui/mod.rs`. The album artist flow has three independent phases:
//! - Canonicalization: Unify variant spellings
//! - Collation: Choose album_artist for multi-artist albums
//! - Population: Set album_artist for tracks missing it
//!
//! Each phase is selected independently and concludes with a review.
//! App methods delegate to these functions.

use crate::config;
use crate::ui::helpers::open_database;
use crate::ui::{App, UiMode};

use super::{
    AlbumArtistBucket, AlbumArtistCanonSession, AlbumArtistClusterAction, AlbumArtistClusterState,
    AlbumArtistPhase, AlbumArtistReviewAction, AlbumArtistReviewState, AlbumArtistVariant,
    CollationAction, CollationReviewAction, CollationReviewState, CollationSession, CollationState,
    PopulationAction, PopulationReviewAction, PopulationReviewState, PopulationSession,
    PopulationState, PhaseSelectorAction, PhaseSelectorState,
};

// ============================================================================
// Flow Entry Point
// ============================================================================

/// Start the album artist resolution flow (phase selector).
pub(crate) fn start(app: &mut App) {
    app.album_artist_phase_selector = Some(PhaseSelectorState::new());
    app.mode = UiMode::AlbumArtistPhaseSelector;
}

/// Handle phase selector actions.
pub(crate) fn handle_phase_action(app: &mut App, action: PhaseSelectorAction) {
    match action {
        PhaseSelectorAction::None => {}
        PhaseSelectorAction::Cancel => {
            app.album_artist_phase_selector = None;
            app.mode = UiMode::MainMenu;
        }
        PhaseSelectorAction::Proceed(phase) => {
            app.album_artist_phase_selector = None;

            // Launch the selected single phase
            match phase {
                AlbumArtistPhase::Canonicalization => start_canonicalization(app),
                AlbumArtistPhase::Collation => launch_collation(app),
                AlbumArtistPhase::Population => launch_population(app),
            }
        }
    }
}

// ============================================================================
// Canonicalization Phase
// ============================================================================

/// Start the canonicalization phase.
pub(crate) fn start_canonicalization(app: &mut App) {
    let db = match open_database(&mut app.status_message) {
        Some(db) => db,
        None => {
            app.mode = UiMode::MainMenu;
            return;
        }
    };

    // Get album_artist buckets with variants
    match db.get_album_artist_canonicalization_buckets() {
        Ok(bucket_data) if !bucket_data.is_empty() => {
            // Convert to AlbumArtistBucket structs
            let buckets: Vec<AlbumArtistBucket> = bucket_data
                .into_iter()
                .map(|(normalized_key, variants)| AlbumArtistBucket {
                    normalized_key,
                    variants: variants
                        .into_iter()
                        .map(|(name, track_count)| AlbumArtistVariant { name, track_count })
                        .collect(),
                })
                .collect();

            let session =
                AlbumArtistCanonSession::new(uuid::Uuid::new_v4().to_string(), buckets);

            app.album_artist_cluster_view = Some(AlbumArtistClusterState::new(session));
            app.mode = UiMode::AlbumArtistClusterView;
        }
        Ok(_) => {
            app.status_message =
                Some("No album_artist name variants found - corpus is clean!".to_string());
            app.mode = UiMode::MainMenu;
        }
        Err(e) => {
            app.status_message = Some(format!("Error loading album_artist buckets: {}", e));
            app.mode = UiMode::MainMenu;
        }
    }
}

/// Handle canonicalization cluster view actions.
pub(crate) fn handle_cluster_action(app: &mut App, action: AlbumArtistClusterAction) {
    match action {
        AlbumArtistClusterAction::None => {}
        AlbumArtistClusterAction::Continue => {}
        AlbumArtistClusterAction::SessionComplete => {
            // All buckets processed - go to review if there are decisions
            if let Some(cluster_view) = app.album_artist_cluster_view.take() {
                let session = cluster_view.into_session();
                if session.decisions.is_empty() {
                    app.status_message = Some("No changes to commit".to_string());
                    app.mode = UiMode::MainMenu;
                } else {
                    app.album_artist_review = Some(AlbumArtistReviewState::new(session));
                    app.mode = UiMode::AlbumArtistReview;
                }
            }
        }
        AlbumArtistClusterAction::ShowSessionReview => {
            // Escape behavior: back if no changes, review if changes
            if let Some(cluster_view) = app.album_artist_cluster_view.take() {
                let session = cluster_view.into_session();
                if session.decisions.is_empty() {
                    // No changes - go back to main menu
                    app.status_message = Some("No changes made".to_string());
                    app.mode = UiMode::MainMenu;
                } else {
                    // Has changes - go to review
                    app.album_artist_review = Some(AlbumArtistReviewState::new(session));
                    app.mode = UiMode::AlbumArtistReview;
                }
            }
        }
        AlbumArtistClusterAction::StatusMessage(msg) => {
            app.status_message = Some(msg);
        }
    }
}

/// Handle canonicalization review actions.
pub(crate) fn handle_review_action(app: &mut App, action: AlbumArtistReviewAction) {
    match action {
        AlbumArtistReviewAction::None => {}
        AlbumArtistReviewAction::Continue => {}
        AlbumArtistReviewAction::Commit => {
            if let Some(review) = app.album_artist_review.take() {
                let session = review.into_session();
                commit_canonicalization_decisions(app, session);
            }
            app.mode = UiMode::MainMenu;
        }
        AlbumArtistReviewAction::Cancel => {
            app.album_artist_review = None;
            app.status_message = Some("Album artist canonicalization cancelled".to_string());
            app.mode = UiMode::MainMenu;
        }
        AlbumArtistReviewAction::BackToClusterView => {
            // Return to cluster view with the session
            if let Some(review) = app.album_artist_review.take() {
                let session = review.into_session();
                app.album_artist_cluster_view = Some(AlbumArtistClusterState::new(session));
                app.mode = UiMode::AlbumArtistClusterView;
            }
        }
    }
}

/// Commit canonicalization decisions to the database.
pub(crate) fn commit_canonicalization_decisions(app: &mut App, session: AlbumArtistCanonSession) {
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
            match db.get_track_ids_by_album_artist(variant_name) {
                Ok(track_ids) => {
                    match db.update_album_artist_for_tracks(&track_ids, &decision.canonical_name) {
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
            "Album artist canonicalization complete: {} tracks updated",
            total_updated
        ));
    } else {
        app.status_message = Some(format!(
            "Album artist canonicalization: {} updated, {} errors",
            total_updated,
            errors.len()
        ));
    }
}

// ============================================================================
// Collation Phase
// ============================================================================

/// Launch the collation phase.
pub(crate) fn launch_collation(app: &mut App) {
    let db = match open_database(&mut app.status_message) {
        Some(db) => db,
        None => {
            app.mode = UiMode::MainMenu;
            return;
        }
    };

    match db.get_albums_with_multiple_artists() {
        Ok(albums) if !albums.is_empty() => {
            let session = CollationSession::new(uuid::Uuid::new_v4().to_string(), albums);
            app.album_artist_collation = Some(CollationState::new(session));
            app.mode = UiMode::AlbumArtistCollation;
        }
        Ok(_) => {
            app.status_message =
                Some("No multi-artist albums found - corpus is clean!".to_string());
            app.mode = UiMode::MainMenu;
        }
        Err(e) => {
            app.status_message = Some(format!("Error loading collation data: {}", e));
            app.mode = UiMode::MainMenu;
        }
    }
}

/// Handle collation actions.
pub(crate) fn handle_collation_action(app: &mut App, action: CollationAction) {
    match action {
        CollationAction::None => {}
        CollationAction::Continue => {}
        CollationAction::SessionComplete => {
            // Collation session complete - go to review if there are decisions
            if let Some(collation_state) = app.album_artist_collation.take() {
                let session = collation_state.into_session();
                if session.decisions.is_empty() {
                    app.status_message = Some("No changes to commit".to_string());
                    app.mode = UiMode::MainMenu;
                } else {
                    app.album_artist_collation_review = Some(CollationReviewState::new(session));
                    app.mode = UiMode::AlbumArtistCollationReview;
                }
            }
        }
        CollationAction::ShowReview => {
            // Escape behavior: back if no changes, review if changes
            if let Some(collation_state) = app.album_artist_collation.take() {
                let session = collation_state.into_session();
                if session.decisions.is_empty() {
                    // No changes - go back to main menu
                    app.status_message = Some("No changes made".to_string());
                    app.mode = UiMode::MainMenu;
                } else {
                    // Has changes - go to review
                    app.album_artist_collation_review = Some(CollationReviewState::new(session));
                    app.mode = UiMode::AlbumArtistCollationReview;
                }
            }
        }
        CollationAction::StatusMessage(msg) => {
            app.status_message = Some(msg);
        }
        CollationAction::Cancel => {
            app.album_artist_collation = None;
            app.status_message = Some("Album artist collation cancelled".to_string());
            app.mode = UiMode::MainMenu;
        }
    }
}

/// Handle collation review actions.
pub(crate) fn handle_collation_review_action(app: &mut App, action: CollationReviewAction) {
    match action {
        CollationReviewAction::None => {}
        CollationReviewAction::Continue => {}
        CollationReviewAction::Commit => {
            // Commit collation decisions
            if let Some(review) = app.album_artist_collation_review.take() {
                let session = review.into_session();
                commit_collation_decisions(app, session);
            }
            app.mode = UiMode::MainMenu;
        }
        CollationReviewAction::Cancel => {
            app.album_artist_collation_review = None;
            app.status_message = Some("Collation review cancelled".to_string());
            app.mode = UiMode::MainMenu;
        }
        CollationReviewAction::BackToCollation => {
            // Go back to collation view with the session
            if let Some(review) = app.album_artist_collation_review.take() {
                let session = review.into_session();
                app.album_artist_collation = Some(CollationState::new(session));
                app.mode = UiMode::AlbumArtistCollation;
            }
        }
    }
}

/// Commit collation decisions to the database.
pub(crate) fn commit_collation_decisions(app: &mut App, session: CollationSession) {
    let db = match open_database(&mut app.status_message) {
        Some(db) => db,
        None => return,
    };

    let mut success_count = 0;
    let mut error_count = 0;

    for decision in &session.decisions {
        // For each collation decision, update album_artist for all tracks on that album
        match db.get_track_ids_by_album(&decision.album_name) {
            Ok(track_ids) => {
                for track_id in track_ids {
                    if let Err(e) =
                        db.update_track_tag(track_id, "album_artist", &decision.chosen_album_artist)
                    {
                        let _ = config::log_message(&format!(
                            "Failed to update album_artist for track {}: {}",
                            track_id, e
                        ));
                        error_count += 1;
                    } else {
                        success_count += 1;
                    }
                }
            }
            Err(e) => {
                let _ = config::log_message(&format!(
                    "Failed to get tracks for album '{}': {}",
                    decision.album_name, e
                ));
                error_count += 1;
            }
        }
    }

    let msg = if error_count > 0 {
        format!(
            "Collation committed: {} tracks updated, {} errors",
            success_count, error_count
        )
    } else {
        format!("Collation committed: {} tracks updated", success_count)
    };
    app.status_message = Some(msg);
}

// ============================================================================
// Population Phase
// ============================================================================

/// Launch the population phase.
pub(crate) fn launch_population(app: &mut App) {
    let db = match open_database(&mut app.status_message) {
        Some(db) => db,
        None => {
            app.mode = UiMode::MainMenu;
            return;
        }
    };

    match db.get_tracks_missing_album_artist() {
        Ok(groups) if !groups.is_empty() => {
            let session = PopulationSession::new(uuid::Uuid::new_v4().to_string(), groups);
            app.album_artist_population = Some(PopulationState::new(session));
            app.mode = UiMode::AlbumArtistPopulation;
        }
        Ok(_) => {
            app.status_message =
                Some("No tracks missing album_artist - corpus is clean!".to_string());
            app.mode = UiMode::MainMenu;
        }
        Err(e) => {
            app.status_message = Some(format!("Error loading population data: {}", e));
            app.mode = UiMode::MainMenu;
        }
    }
}

/// Handle population actions.
pub(crate) fn handle_population_action(app: &mut App, action: PopulationAction) {
    match action {
        PopulationAction::None => {}
        PopulationAction::Continue => {}
        PopulationAction::SessionComplete => {
            // Population session complete - go to review if there are decisions
            if let Some(population_state) = app.album_artist_population.take() {
                let session = population_state.into_session();
                if session.decisions.is_empty() {
                    app.status_message = Some("No changes to commit".to_string());
                    app.mode = UiMode::MainMenu;
                } else {
                    app.album_artist_population_review = Some(PopulationReviewState::new(session));
                    app.mode = UiMode::AlbumArtistPopulationReview;
                }
            }
        }
        PopulationAction::ShowReview => {
            // Escape behavior: back if no changes, review if changes
            if let Some(population_state) = app.album_artist_population.take() {
                let session = population_state.into_session();
                if session.decisions.is_empty() {
                    // No changes - go back to main menu
                    app.status_message = Some("No changes made".to_string());
                    app.mode = UiMode::MainMenu;
                } else {
                    // Has changes - go to review
                    app.album_artist_population_review = Some(PopulationReviewState::new(session));
                    app.mode = UiMode::AlbumArtistPopulationReview;
                }
            }
        }
        PopulationAction::StatusMessage(msg) => {
            app.status_message = Some(msg);
        }
        PopulationAction::Cancel => {
            app.album_artist_population = None;
            app.status_message = Some("Album artist population cancelled".to_string());
            app.mode = UiMode::MainMenu;
        }
    }
}

/// Handle population review actions.
pub(crate) fn handle_population_review_action(app: &mut App, action: PopulationReviewAction) {
    match action {
        PopulationReviewAction::None => {}
        PopulationReviewAction::Continue => {}
        PopulationReviewAction::Commit => {
            // Commit population decisions
            if let Some(review) = app.album_artist_population_review.take() {
                let session = review.into_session();
                commit_population_decisions(app, session);
            }
            app.mode = UiMode::MainMenu;
        }
        PopulationReviewAction::Cancel => {
            app.album_artist_population_review = None;
            app.status_message = Some("Population review cancelled".to_string());
            app.mode = UiMode::MainMenu;
        }
        PopulationReviewAction::BackToPopulation => {
            // Go back to population view with the session
            if let Some(review) = app.album_artist_population_review.take() {
                let session = review.into_session();
                app.album_artist_population = Some(PopulationState::new(session));
                app.mode = UiMode::AlbumArtistPopulation;
            }
        }
    }
}

/// Commit population decisions to the database.
pub(crate) fn commit_population_decisions(app: &mut App, session: PopulationSession) {
    let db = match open_database(&mut app.status_message) {
        Some(db) => db,
        None => return,
    };

    let mut success_count = 0;
    let mut error_count = 0;

    for decision in &session.decisions {
        // Get tracks missing album_artist that match this decision's group
        // We look up by album name if present, otherwise would need directory lookup
        let track_ids = if decision.group_key.starts_with("[album:") {
            // Extract album name from group_key like "[album:Album Name]"
            let album_name = decision
                .group_key
                .strip_prefix("[album:")
                .and_then(|s| s.strip_suffix("]"))
                .unwrap_or(&decision.group_key);
            db.get_track_ids_by_album(album_name).unwrap_or_default()
        } else {
            // For directory-based groups, we'd need a different query
            // For now, skip these
            Vec::new()
        };

        for track_id in track_ids {
            if let Err(e) =
                db.update_track_tag(track_id, "album_artist", &decision.chosen_album_artist)
            {
                let _ = config::log_message(&format!(
                    "Failed to update album_artist for track {}: {}",
                    track_id, e
                ));
                error_count += 1;
            } else {
                success_count += 1;
            }
        }
    }

    let msg = if error_count > 0 {
        format!(
            "Population committed: {} tracks updated, {} errors",
            success_count, error_count
        )
    } else {
        format!("Population committed: {} tracks updated", success_count)
    };
    app.status_message = Some(msg);
}
