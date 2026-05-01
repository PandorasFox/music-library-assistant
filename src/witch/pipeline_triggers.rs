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
//!           → transition_to_idle → TriggerPacking (incremental)
//!             → scoring-only release packing
//!               → ... idle threshold elapses ...
//!                 → TriggerFullRepack
//!                   → full release packing pipeline (mapping/MIS)
//! ```

use std::time::{Duration, Instant};

use crate::meta::recomputation::RecomputationScope;

// ============================================================================
// Idle Transition Decisions
// ============================================================================

/// What the Witch should do when the Done→Idle linger expires (or while idle).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum IdleAction {
    /// Trigger AcoustID fetch for newly-fingerprinted files.
    TriggerFetch,
    /// Trigger incremental release packing (scoring-only) after AcoustID fetch.
    TriggerPacking,
    /// Trigger automatic library deployment (soft mutations).
    TriggerAutoDeploy,
    /// Trigger a full release repack to reconcile mapping/MIS against the
    /// scoring data refreshed by previous incremental passes.
    TriggerFullRepack,
    /// Actually go idle — nothing to auto-trigger.
    GoIdle,
}

/// Idle-promotion bookkeeping passed into `decide_idle_action`.
///
/// `incremental_since_full_repack`: true if at least one incremental
/// release-packing pass has been queued since the last full repack. Cleared
/// when a full repack is queued.
///
/// `last_idle_entry_at`: when the work-state last transitioned into Idle.
/// Cleared whenever the Witch leaves Idle. None means "not currently idle"
/// or "freshly transitioning to idle right now" — in either case the timer
/// hasn't elapsed yet.
///
/// `idle_full_repack_after`: configured idle threshold. `Duration::ZERO`
/// disables auto-promotion entirely (matches the config sentinel of 0).
#[derive(Debug, Clone, Copy)]
pub(super) struct IdlePromotionState {
    pub incremental_since_full_repack: bool,
    pub last_idle_entry_at: Option<Instant>,
    pub idle_full_repack_after: Duration,
}

/// Decide what to do when transitioning to idle (or polling while idle).
///
/// Priority: Fetch > IncrementalPacking > AutoDeploy > FullRepack > GoIdle.
/// Fetch takes priority over packing because packing depends on fetch results.
/// FullRepack only fires once the higher-priority triggers are exhausted —
/// it's an idle-timer-driven catch-up, not a reactive trigger.
/// SIDECAR_DEPLOY is not checked here — it fires eagerly from
/// `transition_to_completed`, never reaching the idle priority chain.
pub(super) fn decide_idle_action(
    pending: super::types::PendingWork,
    has_api_key: bool,
    fetch_active: bool,
    auto_deploy_enabled: bool,
    promotion: IdlePromotionState,
) -> IdleAction {
    use super::types::PendingWork;

    // Fetch takes priority — packing depends on fetch results
    if pending.contains(PendingWork::FETCH) && has_api_key && !fetch_active {
        return IdleAction::TriggerFetch;
    }

    if pending.contains(PendingWork::PACKING) {
        return IdleAction::TriggerPacking;
    }

    if pending.contains(PendingWork::AUDIO_DEPLOY) && auto_deploy_enabled {
        return IdleAction::TriggerAutoDeploy;
    }

    // Idle-timer-driven full repack — mutually exclusive with the reactive
    // TriggerPacking above (PACKING is already cleared at this point).
    if promotion.incremental_since_full_repack
        && !promotion.idle_full_repack_after.is_zero()
        && promotion
            .last_idle_entry_at
            .is_some_and(|t| t.elapsed() >= promotion.idle_full_repack_after)
    {
        return IdleAction::TriggerFullRepack;
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
    use super::super::types::PendingWork;

    /// Promotion state with auto-promotion disabled (the default for tests
    /// that don't care about the idle-timer path).
    fn no_promotion() -> IdlePromotionState {
        IdlePromotionState {
            incremental_since_full_repack: false,
            last_idle_entry_at: None,
            idle_full_repack_after: Duration::ZERO,
        }
    }

    // ---- decide_idle_action ----

    #[test]
    fn idle_action_triggers_fetch_when_files_indexed_and_api_key_present() {
        let mut pw = PendingWork::EMPTY;
        pw.insert(PendingWork::FETCH);
        assert_eq!(decide_idle_action(pw, true, false, false, no_promotion()), IdleAction::TriggerFetch);
    }

    #[test]
    fn idle_action_skips_fetch_when_no_api_key() {
        let mut pw = PendingWork::EMPTY;
        pw.insert(PendingWork::FETCH);
        assert_eq!(decide_idle_action(pw, false, false, false, no_promotion()), IdleAction::GoIdle);
    }

    #[test]
    fn idle_action_skips_fetch_when_fetch_already_active() {
        let mut pw = PendingWork::EMPTY;
        pw.insert(PendingWork::FETCH);
        assert_eq!(decide_idle_action(pw, true, true, false, no_promotion()), IdleAction::GoIdle);
    }

    #[test]
    fn idle_action_fetch_takes_priority_over_packing() {
        let mut pw = PendingWork::EMPTY;
        pw.insert(PendingWork::FETCH | PendingWork::PACKING);
        assert_eq!(decide_idle_action(pw, true, false, false, no_promotion()), IdleAction::TriggerFetch);
    }

    #[test]
    fn idle_action_triggers_packing_when_needed() {
        let mut pw = PendingWork::EMPTY;
        pw.insert(PendingWork::PACKING);
        assert_eq!(decide_idle_action(pw, true, false, false, no_promotion()), IdleAction::TriggerPacking);
    }

    #[test]
    fn idle_action_triggers_packing_without_api_key() {
        let mut pw = PendingWork::EMPTY;
        pw.insert(PendingWork::PACKING);
        assert_eq!(decide_idle_action(pw, false, false, false, no_promotion()), IdleAction::TriggerPacking);
    }

    #[test]
    fn idle_action_goes_idle_when_nothing_needed() {
        assert_eq!(decide_idle_action(PendingWork::EMPTY, true, false, false, no_promotion()), IdleAction::GoIdle);
    }

    #[test]
    fn idle_action_goes_idle_when_nothing_needed_no_api_key() {
        assert_eq!(decide_idle_action(PendingWork::EMPTY, false, false, false, no_promotion()), IdleAction::GoIdle);
    }

    #[test]
    fn idle_action_files_indexed_but_fetch_active_falls_to_packing() {
        let mut pw = PendingWork::EMPTY;
        pw.insert(PendingWork::FETCH | PendingWork::PACKING);
        assert_eq!(decide_idle_action(pw, true, true, false, no_promotion()), IdleAction::TriggerPacking);
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
        let actions = decide_post_fetch_actions(RecomputationScope::TAGS);
        assert!(!actions.set_packing_needed);
        assert_eq!(actions.scope_for_content_analysis, None);
    }

    #[test]
    fn post_fetch_triggers_on_combined_scope_with_external() {
        let scope = RecomputationScope::EXTERNAL | RecomputationScope::TAGS;
        let actions = decide_post_fetch_actions(scope);
        assert!(actions.set_packing_needed);
        assert_eq!(actions.scope_for_content_analysis, Some(scope));
    }

    #[test]
    fn post_fetch_preserves_full_scope_for_content_analysis() {
        let scope = RecomputationScope::EXTERNAL
            | RecomputationScope::TAGS
            | RecomputationScope::FILES
            | RecomputationScope::DEPLOY;
        let actions = decide_post_fetch_actions(scope);
        assert!(actions.set_packing_needed);
        assert_eq!(actions.scope_for_content_analysis, Some(scope));
    }

    // ---- auto-deploy ----

    #[test]
    fn idle_action_triggers_auto_deploy_when_needed_and_enabled() {
        let mut pw = PendingWork::EMPTY;
        pw.insert(PendingWork::AUDIO_DEPLOY);
        assert_eq!(decide_idle_action(pw, false, false, true, no_promotion()), IdleAction::TriggerAutoDeploy);
    }

    #[test]
    fn idle_action_auto_deploy_skipped_when_not_enabled() {
        let mut pw = PendingWork::EMPTY;
        pw.insert(PendingWork::AUDIO_DEPLOY);
        assert_eq!(decide_idle_action(pw, false, false, false, no_promotion()), IdleAction::GoIdle);
    }

    #[test]
    fn idle_action_auto_deploy_skipped_when_not_needed() {
        assert_eq!(decide_idle_action(PendingWork::EMPTY, false, false, true, no_promotion()), IdleAction::GoIdle);
    }

    #[test]
    fn idle_action_fetch_takes_priority_over_auto_deploy() {
        let mut pw = PendingWork::EMPTY;
        pw.insert(PendingWork::FETCH | PendingWork::AUDIO_DEPLOY);
        assert_eq!(decide_idle_action(pw, true, false, true, no_promotion()), IdleAction::TriggerFetch);
    }

    #[test]
    fn idle_action_packing_takes_priority_over_auto_deploy() {
        let mut pw = PendingWork::EMPTY;
        pw.insert(PendingWork::PACKING | PendingWork::AUDIO_DEPLOY);
        assert_eq!(decide_idle_action(pw, false, false, true, no_promotion()), IdleAction::TriggerPacking);
    }

    #[test]
    fn sidecar_deploy_not_visible_to_idle_action() {
        // SIDECAR_DEPLOY is consumed eagerly, never reaches idle action
        let mut pw = PendingWork::EMPTY;
        pw.insert(PendingWork::SIDECAR_DEPLOY);
        assert_eq!(decide_idle_action(pw, false, false, true, no_promotion()), IdleAction::GoIdle);
    }

    // ---- idle full-repack promotion ----

    /// Build a promotion state that *would* trigger if the elapsed-time
    /// guard passes. Caller controls how far in the past `last_idle_entry_at`
    /// is by passing a `Duration` that's been subtracted from `Instant::now()`.
    fn promotion_eligible(elapsed_since_idle: Duration, threshold: Duration) -> IdlePromotionState {
        IdlePromotionState {
            incremental_since_full_repack: true,
            // Instant::checked_sub is None for huge durations, but our test
            // values are small (seconds). unwrap_or(now) is a safe fallback.
            last_idle_entry_at: Instant::now().checked_sub(elapsed_since_idle),
            idle_full_repack_after: threshold,
        }
    }

    #[test]
    fn idle_action_does_not_promote_when_flag_is_false() {
        // (a) Even after a long idle, if no incremental ran since the last
        // full repack, GoIdle is correct.
        let promotion = IdlePromotionState {
            incremental_since_full_repack: false,
            last_idle_entry_at: Instant::now().checked_sub(Duration::from_secs(3600)),
            idle_full_repack_after: Duration::from_secs(60),
        };
        assert_eq!(
            decide_idle_action(PendingWork::EMPTY, false, false, false, promotion),
            IdleAction::GoIdle,
        );
    }

    #[test]
    fn idle_action_does_not_promote_when_idle_too_short() {
        // (b) Flag is true but elapsed < threshold → still GoIdle.
        let promotion = promotion_eligible(
            Duration::from_secs(5),
            Duration::from_secs(60),
        );
        assert_eq!(
            decide_idle_action(PendingWork::EMPTY, false, false, false, promotion),
            IdleAction::GoIdle,
        );
    }

    #[test]
    fn idle_action_promotes_full_repack_when_threshold_elapsed() {
        // (c) Both conditions met → TriggerFullRepack.
        let promotion = promotion_eligible(
            Duration::from_secs(120),
            Duration::from_secs(60),
        );
        assert_eq!(
            decide_idle_action(PendingWork::EMPTY, false, false, false, promotion),
            IdleAction::TriggerFullRepack,
        );
    }

    #[test]
    fn idle_action_does_not_promote_when_disabled() {
        // Threshold of 0 disables auto-promotion entirely.
        let promotion = IdlePromotionState {
            incremental_since_full_repack: true,
            last_idle_entry_at: Instant::now().checked_sub(Duration::from_secs(3600)),
            idle_full_repack_after: Duration::ZERO,
        };
        assert_eq!(
            decide_idle_action(PendingWork::EMPTY, false, false, false, promotion),
            IdleAction::GoIdle,
        );
    }

    #[test]
    fn idle_action_does_not_promote_when_not_yet_idle() {
        // last_idle_entry_at = None means we haven't entered Idle yet —
        // the promotion timer must not fire.
        let promotion = IdlePromotionState {
            incremental_since_full_repack: true,
            last_idle_entry_at: None,
            idle_full_repack_after: Duration::from_secs(60),
        };
        assert_eq!(
            decide_idle_action(PendingWork::EMPTY, false, false, false, promotion),
            IdleAction::GoIdle,
        );
    }

    #[test]
    fn idle_action_fetch_takes_priority_over_full_repack() {
        // (d) Higher-priority triggers win when both are eligible.
        let mut pw = PendingWork::EMPTY;
        pw.insert(PendingWork::FETCH);
        let promotion = promotion_eligible(
            Duration::from_secs(120),
            Duration::from_secs(60),
        );
        assert_eq!(
            decide_idle_action(pw, true, false, false, promotion),
            IdleAction::TriggerFetch,
        );
    }

    #[test]
    fn idle_action_incremental_packing_takes_priority_over_full_repack() {
        // (d) PACKING (incremental, reactive) beats the idle-timer full repack.
        let mut pw = PendingWork::EMPTY;
        pw.insert(PendingWork::PACKING);
        let promotion = promotion_eligible(
            Duration::from_secs(120),
            Duration::from_secs(60),
        );
        assert_eq!(
            decide_idle_action(pw, false, false, false, promotion),
            IdleAction::TriggerPacking,
        );
    }

    #[test]
    fn idle_action_auto_deploy_takes_priority_over_full_repack() {
        // (d) AUDIO_DEPLOY also beats the idle-timer full repack.
        let mut pw = PendingWork::EMPTY;
        pw.insert(PendingWork::AUDIO_DEPLOY);
        let promotion = promotion_eligible(
            Duration::from_secs(120),
            Duration::from_secs(60),
        );
        assert_eq!(
            decide_idle_action(pw, false, false, true, promotion),
            IdleAction::TriggerAutoDeploy,
        );
    }
}
