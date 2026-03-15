//! Generic dispatch for resolution modals.
//!
//! The `Dispatchable` trait maps button actions to `DispatchResult`s
//! (mutations + decision key). This moves mutation-building logic from
//! mm-tui action handlers into mm-ui, making it available to both TUI
//! and web clients.
//!
//! The TUI uses a `dispatch_resolution!` macro to generate one-line
//! `HandleAction` impls that delegate to `Dispatchable::dispatch()`.

use mm_meta::decisions::DecisionKey;
use mm_meta::mutations::Mutation;
use mm_meta::paths::PathResolver;

/// Result of dispatching a button action on a resolution modal.
#[derive(Debug)]
pub enum DispatchResult {
    /// Stage these mutations as a decision, then advance to next group.
    Stage {
        key: DecisionKey,
        label: String,
        mutations: Vec<Mutation>,
    },
    /// Stage mutations but don't advance.
    ///
    /// Used for per-item actions (e.g., ManualReview stash) where the
    /// modal stays on the current group after staging.
    StageKeep {
        key: DecisionKey,
        label: String,
        mutations: Vec<Mutation>,
    },
    /// Advance to next group without staging any mutations.
    ///
    /// Used for skip actions (e.g., DiscExtraction skip).
    Skip,
    /// Cancel the entire resolution flow.
    Cancel,
    /// No-op (empty mutations, invalid state, etc.).
    Handled,
}

/// Trait for resolution states that can dispatch button actions to mutations.
///
/// Moves mutation-building logic from mm-tui action handlers into mm-ui,
/// making it available to both TUI and web clients. Implemented on
/// `ResolutionState<D, B>` for no-field modals, or on concrete ViewState
/// structs for field modals.
pub trait Dispatchable {
    /// The action type produced by this modal's buttons.
    type Action;

    /// Map a button action to a dispatch result.
    ///
    /// `resolver` is provided for mutations that need absolute path
    /// resolution (stash operations). Mutation builders that don't need
    /// paths can ignore it.
    fn dispatch(&self, action: Self::Action, resolver: &PathResolver) -> DispatchResult;

    /// Advance to the next group/cluster after staging.
    ///
    /// Returns `true` if there are more groups to process.
    /// Returns `false` when the last group has been processed (caller
    /// should transition to review).
    ///
    /// Default: `false` (single-step resolutions go straight to review).
    fn advance(&mut self) -> bool {
        false
    }

    /// Cancel log message.
    fn cancel_message(&self) -> &'static str {
        "Resolution cancelled"
    }
}
