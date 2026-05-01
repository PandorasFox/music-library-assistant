//! VariousArtistsOverride application builder.
//!
//! Pure function: takes operator-approved override applications + each
//! release's packed inodes + their current tags, produces `ApprovalDecision`s
//! that rewrite `ALBUMARTIST` to the chosen value. Mirrors `approval.rs` —
//! callable from any context (TUI, web, in-witch batch handler).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::mutations::TagOp;
use crate::views::external_matches::ApprovalDecision;

/// Per-release input for `build_va_override_decisions`.
///
/// `release_id` matches a `signal_various_artists_override` row;
/// `albumartist` is the operator-chosen `ALBUMARTIST` value (defaulting to
/// the signal's `suggested_artist`, optionally edited in the review UI);
/// `inodes` are every inode currently packed to that release per
/// `signal_release_packing`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaOverrideInput {
    pub release_id: String,
    pub albumartist: String,
    pub inodes: Vec<i64>,
}

/// Per-release/per-inode breakdown of a VA-override apply pass.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaOverrideSummary {
    /// Releases that produced at least one decision.
    pub staged_releases: usize,
    /// Total inodes that received a tag op across all decisions.
    pub staged_inodes: usize,
    /// Releases dropped because no inodes were packed to them.
    pub skipped_releases: usize,
    /// Inodes whose current ALBUMARTIST already matched the chosen value
    /// (no-op — no tag op emitted).
    pub already_matching_inodes: usize,
}

/// Build VA-override decisions from selected applications + inode-tag context.
///
/// Pure function. For each application:
/// - skips releases with no packed inodes (counts as skipped),
/// - per inode, compares current `ALBUMARTIST` value(s) against the chosen
///   one and emits the minimal set of `TagOp`s to converge:
///   - if no `ALBUMARTIST` exists: Add(new),
///   - if exactly one and it matches: no-op (counts as already_matching),
///   - if exactly one and it differs: Replace(old, new),
///   - if multiple values: Drop every current value, Add(new).
pub fn build_va_override_decisions(
    applications: &[VaOverrideInput],
    inode_tags: &HashMap<i64, Vec<(String, String)>>,
) -> (Vec<ApprovalDecision>, VaOverrideSummary) {
    let mut decisions = Vec::new();
    let mut summary = VaOverrideSummary::default();

    for app in applications {
        if app.inodes.is_empty() {
            summary.skipped_releases += 1;
            continue;
        }

        let mut per_inode_ops: Vec<Vec<TagOp>> = Vec::new();
        for inode in &app.inodes {
            let current_albumartists: Vec<String> = inode_tags
                .get(inode)
                .map(|tags| {
                    tags.iter()
                        .filter(|(k, _)| k.eq_ignore_ascii_case("ALBUMARTIST"))
                        .map(|(_, v)| v.clone())
                        .collect()
                })
                .unwrap_or_default();

            let ops = match current_albumartists.as_slice() {
                [] => vec![TagOp {
                    inode: *inode,
                    tag_name: "ALBUMARTIST".to_string(),
                    old_value: None,
                    new_value: Some(app.albumartist.clone()),
                }],
                [only] if only == &app.albumartist => {
                    summary.already_matching_inodes += 1;
                    Vec::new()
                }
                [only] => vec![TagOp {
                    inode: *inode,
                    tag_name: "ALBUMARTIST".to_string(),
                    old_value: Some(only.clone()),
                    new_value: Some(app.albumartist.clone()),
                }],
                multi => {
                    let mut ops: Vec<TagOp> = multi
                        .iter()
                        .map(|v| TagOp {
                            inode: *inode,
                            tag_name: "ALBUMARTIST".to_string(),
                            old_value: Some(v.clone()),
                            new_value: None,
                        })
                        .collect();
                    ops.push(TagOp {
                        inode: *inode,
                        tag_name: "ALBUMARTIST".to_string(),
                        old_value: None,
                        new_value: Some(app.albumartist.clone()),
                    });
                    ops
                }
            };

            if !ops.is_empty() {
                per_inode_ops.push(ops);
            }
        }

        if per_inode_ops.is_empty() {
            // Every inode either had no ops to emit (already matching) or
            // wasn't found in tags. If skipped due to all-already-matching,
            // this release didn't need a decision; if everything was missing
            // the release fell through with empty `current_albumartists` and
            // produced an Add op, so this branch only fires when every
            // inode was already_matching.
            summary.skipped_releases += 1;
            continue;
        }

        let label = format!(
            "Apply VA override: {}",
            &app.release_id[..8.min(app.release_id.len())]
        );
        summary.staged_inodes += per_inode_ops.len();
        decisions.push(ApprovalDecision {
            release_id: app.release_id.clone(),
            label,
            per_inode_ops,
        });
    }

    summary.staged_releases = decisions.len();
    (decisions, summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn missing_albumartist_emits_add() {
        let app = VaOverrideInput {
            release_id: "abcd1234-...".to_string(),
            albumartist: "Yasunori Mitsuda".to_string(),
            inodes: vec![1],
        };
        let mut inode_tags = HashMap::new();
        inode_tags.insert(1, tags(&[("TITLE", "Schala's Theme")]));
        let (decisions, summary) = build_va_override_decisions(&[app], &inode_tags);
        assert_eq!(decisions.len(), 1);
        assert_eq!(summary.staged_inodes, 1);
        assert_eq!(summary.already_matching_inodes, 0);
        let ops = &decisions[0].per_inode_ops[0];
        assert_eq!(ops.len(), 1);
        assert!(ops[0].old_value.is_none());
        assert_eq!(ops[0].new_value.as_deref(), Some("Yasunori Mitsuda"));
    }

    #[test]
    fn single_value_replace() {
        let app = VaOverrideInput {
            release_id: "rid".to_string(),
            albumartist: "New Artist".to_string(),
            inodes: vec![1],
        };
        let mut inode_tags = HashMap::new();
        inode_tags.insert(1, tags(&[("ALBUMARTIST", "Various Artists")]));
        let (decisions, _) = build_va_override_decisions(&[app], &inode_tags);
        let ops = &decisions[0].per_inode_ops[0];
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].old_value.as_deref(), Some("Various Artists"));
        assert_eq!(ops[0].new_value.as_deref(), Some("New Artist"));
    }

    #[test]
    fn already_matching_skips() {
        let app = VaOverrideInput {
            release_id: "rid".to_string(),
            albumartist: "Already Set".to_string(),
            inodes: vec![1],
        };
        let mut inode_tags = HashMap::new();
        inode_tags.insert(1, tags(&[("ALBUMARTIST", "Already Set")]));
        let (decisions, summary) = build_va_override_decisions(&[app], &inode_tags);
        assert!(decisions.is_empty());
        assert_eq!(summary.already_matching_inodes, 1);
        assert_eq!(summary.skipped_releases, 1);
    }

    #[test]
    fn multi_value_drops_all_then_adds() {
        let app = VaOverrideInput {
            release_id: "rid".to_string(),
            albumartist: "Singular".to_string(),
            inodes: vec![1],
        };
        let mut inode_tags = HashMap::new();
        inode_tags.insert(
            1,
            tags(&[
                ("ALBUMARTIST", "First"),
                ("ALBUMARTIST", "Second"),
            ]),
        );
        let (decisions, _) = build_va_override_decisions(&[app], &inode_tags);
        let ops = &decisions[0].per_inode_ops[0];
        // 2 drops + 1 add
        assert_eq!(ops.len(), 3);
        assert_eq!(
            ops.iter().filter(|o| o.old_value.is_some() && o.new_value.is_none()).count(),
            2
        );
        assert_eq!(
            ops.iter().filter(|o| o.old_value.is_none() && o.new_value.is_some()).count(),
            1
        );
    }

    #[test]
    fn empty_inodes_skips_release() {
        let app = VaOverrideInput {
            release_id: "rid".to_string(),
            albumartist: "X".to_string(),
            inodes: vec![],
        };
        let (decisions, summary) = build_va_override_decisions(&[app], &HashMap::new());
        assert!(decisions.is_empty());
        assert_eq!(summary.skipped_releases, 1);
    }
}
