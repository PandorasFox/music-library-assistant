//! Pure decision logic for the automated pipeline triggers.
//!
//! Extracts the "what should happen" decisions from the Witch's state machine
//! into testable pure functions. The Witch calls these and applies the results.
//!
//! Pipeline chain: index → fetch → content analysis → pack
//!
//! ```text
//! New files indexed (files_indexed_this_cycle = true)
//!   → transition_to_idle → TriggerFetch
//!     → AcoustID + MB enrichment (external fetch scheduler)
//!       → AllDone with EXTERNAL scope
//!         → queue ScheduleContentAnalysis + set packing_needed
//!           → transition_to_idle → TriggerPacking
//!             → full release packing pipeline
//! ```

use crate::meta::recomputation::RecomputationScope;

// ============================================================================
// Idle Transition Decisions
// ============================================================================

/// What the Witch should do when the Done→Idle linger expires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum IdleAction {
    /// Trigger AcoustID fetch for newly-fingerprinted files.
    TriggerFetch,
    /// Trigger full release packing pipeline (MIS).
    TriggerPacking,
    /// Actually go idle — nothing to auto-trigger.
    GoIdle,
}

/// Decide what to do when transitioning to idle.
///
/// Fetch takes priority over packing because packing depends on fetch results.
/// The `files_indexed_this_cycle` flag is always cleared by the caller regardless
/// of the action returned (even if fetch can't trigger due to missing API key,
/// we don't want to retry every 30s).
pub(super) fn decide_idle_action(
    files_indexed_this_cycle: bool,
    packing_needed: bool,
    has_api_key: bool,
    fetch_active: bool,
) -> IdleAction {
    // Fetch takes priority — packing depends on fetch results
    if files_indexed_this_cycle && has_api_key && !fetch_active {
        return IdleAction::TriggerFetch;
    }

    if packing_needed {
        return IdleAction::TriggerPacking;
    }

    IdleAction::GoIdle
}

// ============================================================================
// Post-Fetch Decisions
// ============================================================================

/// What the Witch should do when the external fetch scheduler reports AllDone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PostFetchActions {
    /// If set, queue ScheduleContentAnalysis with this scope.
    pub scope_for_content_analysis: Option<RecomputationScope>,
    /// Whether to set the packing_needed flag.
    pub set_packing_needed: bool,
}

/// Decide what to do after an external fetch batch completes.
///
/// If the session accumulated EXTERNAL scope (meaning AcoustID/MB found new data),
/// we need to derive ExternalMatch signals via content analysis, and flag that
/// release packing should run when the system next goes idle.
pub(super) fn decide_post_fetch_actions(
    session_scope: RecomputationScope,
) -> PostFetchActions {
    if session_scope.contains(RecomputationScope::EXTERNAL) {
        PostFetchActions {
            scope_for_content_analysis: Some(session_scope),
            set_packing_needed: true,
        }
    } else {
        PostFetchActions {
            scope_for_content_analysis: None,
            set_packing_needed: false,
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ---- decide_idle_action ----

    #[test]
    fn idle_action_triggers_fetch_when_files_indexed_and_api_key_present() {
        assert_eq!(
            decide_idle_action(true, false, true, false),
            IdleAction::TriggerFetch,
        );
    }

    #[test]
    fn idle_action_skips_fetch_when_no_api_key() {
        // No API key → can't fetch, fall through to GoIdle
        assert_eq!(
            decide_idle_action(true, false, false, false),
            IdleAction::GoIdle,
        );
    }

    #[test]
    fn idle_action_skips_fetch_when_fetch_already_active() {
        assert_eq!(
            decide_idle_action(true, false, true, true),
            IdleAction::GoIdle,
        );
    }

    #[test]
    fn idle_action_fetch_takes_priority_over_packing() {
        // Both flags set — fetch first, packing will happen on a later idle
        assert_eq!(
            decide_idle_action(true, true, true, false),
            IdleAction::TriggerFetch,
        );
    }

    #[test]
    fn idle_action_triggers_packing_when_needed() {
        assert_eq!(
            decide_idle_action(false, true, true, false),
            IdleAction::TriggerPacking,
        );
    }

    #[test]
    fn idle_action_triggers_packing_without_api_key() {
        // Packing doesn't need an API key — data is already cached
        assert_eq!(
            decide_idle_action(false, true, false, false),
            IdleAction::TriggerPacking,
        );
    }

    #[test]
    fn idle_action_goes_idle_when_nothing_needed() {
        assert_eq!(
            decide_idle_action(false, false, true, false),
            IdleAction::GoIdle,
        );
    }

    #[test]
    fn idle_action_goes_idle_when_nothing_needed_no_api_key() {
        assert_eq!(
            decide_idle_action(false, false, false, false),
            IdleAction::GoIdle,
        );
    }

    #[test]
    fn idle_action_files_indexed_but_fetch_active_falls_to_packing() {
        // Can't fetch (active), but packing is needed
        assert_eq!(
            decide_idle_action(true, true, true, true),
            IdleAction::TriggerPacking,
        );
    }

    // ---- decide_post_fetch_actions ----

    #[test]
    fn post_fetch_triggers_on_external_scope() {
        let scope = RecomputationScope::EXTERNAL;
        let actions = decide_post_fetch_actions(scope);
        assert!(actions.set_packing_needed);
        assert_eq!(actions.scope_for_content_analysis, Some(scope));
    }

    #[test]
    fn post_fetch_no_action_on_empty_scope() {
        let actions = decide_post_fetch_actions(RecomputationScope::EMPTY);
        assert!(!actions.set_packing_needed);
        assert_eq!(actions.scope_for_content_analysis, None);
    }

    #[test]
    fn post_fetch_no_action_on_non_external_scope() {
        // TAGS scope without EXTERNAL — no packing needed
        let actions = decide_post_fetch_actions(RecomputationScope::TAGS);
        assert!(!actions.set_packing_needed);
        assert_eq!(actions.scope_for_content_analysis, None);
    }

    #[test]
    fn post_fetch_triggers_on_combined_scope_with_external() {
        // EXTERNAL | TAGS — still triggers, passes full scope through
        let scope = RecomputationScope::EXTERNAL | RecomputationScope::TAGS;
        let actions = decide_post_fetch_actions(scope);
        assert!(actions.set_packing_needed);
        assert_eq!(actions.scope_for_content_analysis, Some(scope));
    }

    #[test]
    fn post_fetch_preserves_full_scope_for_content_analysis() {
        // All bits set — content analysis gets the full scope
        let scope = RecomputationScope::EXTERNAL
            | RecomputationScope::TAGS
            | RecomputationScope::FILES
            | RecomputationScope::DEPLOY;
        let actions = decide_post_fetch_actions(scope);
        assert!(actions.set_packing_needed);
        assert_eq!(actions.scope_for_content_analysis, Some(scope));
    }
}
