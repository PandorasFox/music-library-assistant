//! Shared release approval builder.
//!
//! Pure function: takes selected release data + MB cache + config,
//! produces `ApprovalDecision`s. Both TUI and web call this identically.
//! The client-specific code handles staging (gesture exchange in TUI,
//! transaction API in web).

use std::collections::HashMap;

use mm_meta::config::CreditRoutingConfig;
use mm_meta::external::musicbrainz::MbCacheBundle;
use mm_meta::external::tag_generation::{generate_tag_ops, MbTagInput};
use mm_meta::mutations::TagOp;
use mm_meta::views::external_matches::{ApprovalDecision, ReleaseApprovalInput};

/// Build approval decisions from selected releases + staging data.
///
/// Pure function — both TUI and web call this with the same inputs
/// and get the same outputs. Client code then stages the decisions
/// using their own mechanisms.
///
/// Returns `(approved, skipped)` — approved decisions + count of
/// tracks skipped due to missing MB cache data.
pub fn build_release_approval_decisions(
    releases: &[ReleaseApprovalInput],
    bundle: &MbCacheBundle,
    inode_tags: &HashMap<i64, Vec<(String, String)>>,
    locales: &[String],
    routing: &CreditRoutingConfig,
) -> (Vec<ApprovalDecision>, usize) {
    let mut decisions = Vec::new();
    let mut skipped = 0usize;

    for rd in releases {
        let Some(release) = bundle.releases.get(&rd.release_id) else {
            skipped += rd.tracks.len();
            continue;
        };
        let total_media = release.media.len() as u32;

        let mut per_inode_ops: Vec<Vec<TagOp>> = Vec::new();
        for t in &rd.tracks {
            let Some(recording) = bundle.recordings.get(&t.recording_id) else {
                skipped += 1;
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
            );
            if !inode_ops.is_empty() {
                per_inode_ops.push(inode_ops);
            }
        }

        if per_inode_ops.is_empty() {
            continue;
        }

        let label = format!(
            "Approve MB release: {}",
            &rd.release_id[..8.min(rd.release_id.len())]
        );
        decisions.push(ApprovalDecision {
            release_id: rd.release_id.clone(),
            label,
            per_inode_ops,
        });
    }

    (decisions, skipped)
}
