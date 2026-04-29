//! Shared release approval builder.
//!
//! Pure function: takes selected release data + MB cache + config,
//! produces `ApprovalDecision`s. Callable from any context (TUI, web,
//! or in-witch batch handler) — no UI dependencies.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::config::{CreditRoutingConfig, MbTagNameConfig};
use crate::external::musicbrainz::MbCacheBundle;
use crate::external::tag_generation::{generate_tag_ops, MbTagInput};
use crate::mutations::TagOp;
use crate::views::external_matches::{ApprovalDecision, ReleaseApprovalInput};

/// Per-release/per-track breakdown of an approval build pass.
///
/// `staged_releases` is the count of decisions produced (one per release
/// that successfully generated at least one tag op). `staged_tracks` is the
/// total number of per-inode tag-op groups across all decisions. The
/// skipped counts capture data that wasn't usable due to missing MB cache.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalSummary {
    /// Releases that produced at least one decision.
    pub staged_releases: usize,
    /// Total individual track-tag groups staged across all decisions.
    pub staged_tracks: usize,
    /// Tracks dropped because their MB recording cache entry was missing.
    pub skipped_tracks: usize,
    /// Releases dropped because their MB release cache entry was missing OR
    /// every track in them had a missing recording (so no decision could be
    /// produced).
    pub skipped_releases: usize,
}

/// Build approval decisions from selected releases + staging data.
///
/// Pure function — TUI, web, and the in-witch batch handler call this
/// with the same inputs and get the same outputs. Returns the decisions
/// plus a summary of staged/skipped counts at both release and track
/// granularity (so callers can render an unambiguous status line).
pub fn build_release_approval_decisions(
    releases: &[ReleaseApprovalInput],
    bundle: &MbCacheBundle,
    inode_tags: &HashMap<i64, Vec<(String, String)>>,
    locales: &[String],
    routing: &CreditRoutingConfig,
    tag_names: &MbTagNameConfig,
) -> (Vec<ApprovalDecision>, ApprovalSummary) {
    let mut decisions = Vec::new();
    let mut summary = ApprovalSummary::default();

    for rd in releases {
        let Some(release) = bundle.releases.get(&rd.release_id) else {
            summary.skipped_releases += 1;
            summary.skipped_tracks += rd.tracks.len();
            continue;
        };
        let total_media = release.media.len() as u32;

        let mut per_inode_ops: Vec<Vec<TagOp>> = Vec::new();
        for t in &rd.tracks {
            let Some(recording) = bundle.recordings.get(&t.recording_id) else {
                summary.skipped_tracks += 1;
                continue;
            };
            let input = MbTagInput {
                inode: t.inode,
                recording_id: t.recording_id.clone(),
                release_id: rd.release_id.clone(),
                track_title: t.track_title.clone(),
                track_position: t.track_position,
                medium_position: t.medium_position,
                total_media,
                current_tags: inode_tags.get(&t.inode).cloned().unwrap_or_default(),
            };
            let inode_ops = generate_tag_ops(
                &input,
                recording,
                release,
                &bundle.artists,
                locales,
                routing,
                tag_names,
            );
            if !inode_ops.is_empty() {
                per_inode_ops.push(inode_ops);
            }
        }

        if per_inode_ops.is_empty() {
            // All tracks had missing recording cache (or generated no ops).
            // Counts as a skipped release; per-track increments already happened above.
            summary.skipped_releases += 1;
            continue;
        }

        let label = format!(
            "Approve MB release: {}",
            &rd.release_id[..8.min(rd.release_id.len())]
        );
        summary.staged_tracks += per_inode_ops.len();
        decisions.push(ApprovalDecision {
            release_id: rd.release_id.clone(),
            label,
            per_inode_ops,
        });
    }

    summary.staged_releases = decisions.len();
    (decisions, summary)
}
