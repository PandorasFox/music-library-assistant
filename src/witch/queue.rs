//! Task queueing machinery for the Witch.
//!
//! Centralizes the enqueue/spawn/label-resolution pipeline used by all
//! queue methods (mutations, computations, maintenance). Also provides
//! the shared `read_config` accessor.
//!
//! This module is part of the Witch subsystem. See `witch/mod.rs` for overview.

use crate::config::Config;
use crate::meta::computations::Computation;
use crate::meta::mutations::Mutation;

use super::types::{SpawnedMutation, Task, TaskLabel};
use super::WorkState;

impl super::Witch {
    // -------------------------------------------------------------------------
    // Config Access Helper
    // -------------------------------------------------------------------------

    /// Read from config snapshot (lock-free via ArcSwap).
    ///
    /// Returns None if config isn't set yet (AwaitingSetup).
    pub(super) fn read_config<T>(&self, f: impl FnOnce(&Config) -> T) -> Option<T> {
        let guard = self.config_snapshot.load();
        guard.as_deref().map(|cfg| f(cfg))
    }

    // -------------------------------------------------------------------------
    // Task Spawning
    // -------------------------------------------------------------------------

    /// Spawn a task on the rayon thread pool with panic catching.
    ///
    /// If the task panics, we still send a failure result so the Witch's
    /// in_flight counter stays accurate and we don't lose tasks silently.
    fn spawn_task(&self, task: Task, label: String) {
        self.hades.dispatch(task, label);
    }

    /// Resolve label, bump counters, and spawn a single task on the rayon pool.
    ///
    /// Consolidates the 4-step sequence (inc_queued / inc_in_flight / inc_label /
    /// spawn_task) used by every queue method.
    pub(super) fn enqueue_one(&mut self, task: Task, label: Option<String>) {
        let task_label = self.resolve_label(label, &task);
        self.work_state.inc_queued(1);
        self.work_state.inc_in_flight();
        self.work_state.inc_label(&task_label);
        self.spawn_task(task, task_label);
    }

    /// Resolve task label from explicit label, current_label, or task fallback.
    fn resolve_label(&self, explicit_label: Option<String>, task: &Task) -> String {
        explicit_label
            .or_else(|| {
                if let WorkState::Working { ref label, .. } = self.work_state {
                    label.clone()
                } else {
                    None
                }
            })
            .unwrap_or_else(|| TaskLabel::from_task(task).0)
    }

    // -------------------------------------------------------------------------
    // Internal Mutation Queueing (used by transaction API)
    // -------------------------------------------------------------------------

    // NOTE: Direct mutation queueing methods (queue, queue_all, etc.) were removed.
    // All mutations must go through the transaction API:
    //   1. start_transaction(label)
    //   2. add_decision(idx, witness, label, mutations) for each decision
    //   3. confirm_transaction(witness) to commit, or discard_transaction(witness) to abort
    //
    // This ensures proper decision witness semantics where each user action is
    // explicitly witnessed, and batch review/commit is possible.

    pub(super) fn queue_mutations_internal(
        &mut self,
        mutations: impl IntoIterator<Item = Mutation>,
        label: Option<String>,
    ) {
        self.transition_to_working();

        let mutations: Vec<_> = mutations.into_iter().collect();

        crate::logging::log_general(format!(
            "[WORKER] queue_mutations_internal: queueing {} mutations (label={:?})",
            mutations.len(),
            label
        ));

        // Store label in WorkState for phase advancement
        if let WorkState::Working {
            label: ref mut ws_label,
            ..
        } = self.work_state
        {
            if ws_label.is_none() {
                *ws_label = label.clone();
            }
        }

        for mutation in mutations {
            self.enqueue_one(Task::Mutation(Box::new(mutation)), label.clone());
        }
    }

    /// Queue a mutation spawned by another mutation (spawn chaining).
    ///
    /// This is called internally from tick() when processing spawn_mutations
    /// from completed tasks. The spawn chain is already authorized by the
    /// parent mutation's witness - no additional operator decision required.
    ///
    /// Example: ApplyTagOps spawns ApplyDbTagsToDisk after DB write succeeds.
    pub(super) fn queue_spawned_mutation(&mut self, spawned: SpawnedMutation) {
        // Extract the inner mutation - SpawnedMutation's existence proves authorization
        let mutation = spawned.into_inner();

        // Spawned mutations inherit the working state from their parent
        // (transition_to_working already happened when parent was queued)
        //
        // Pass None for label so resolve_label falls through to TaskLabel::from_task
        self.enqueue_one(Task::Mutation(Box::new(mutation)), None);
    }

    // -------------------------------------------------------------------------
    // Computation Queueing (internal only, no witness required)
    // -------------------------------------------------------------------------

    /// Queue a single computation with an optional label.
    ///
    /// Computations are derived facts that don't alter state - they only emit
    /// signals. They can execute without user decisions.
    pub(super) fn queue_computation_with_label(&mut self, computation: Computation, label: Option<String>) {
        self.transition_to_working();
        self.enqueue_one(Task::Computation(computation), label);
    }

    // -------------------------------------------------------------------------
    // Soft Mutation Queueing (no transaction required)
    // -------------------------------------------------------------------------

    /// Queue soft mutations with phase-based ordering.
    ///
    /// Soft mutations are bucketed by `SoftMutationPhase` (Cleanup before Deploy)
    /// and executed with drain barriers between phases. The first phase is queued
    /// immediately; remaining phases are stashed for advancement in `update_state()`.
    pub(super) fn queue_soft_mutations_internal(
        &mut self,
        soft_mutations: Vec<mm_meta::soft_mutations::SoftMutation>,
        label: Option<String>,
    ) {
        use std::collections::BTreeMap;
        use mm_meta::soft_mutations::SoftMutationPhase;

        self.transition_to_working();

        let total = soft_mutations.len();
        crate::logging::log_general(format!(
            "[WORKER] queue_soft_mutations_internal: queueing {} soft mutations (label={:?})",
            total, label
        ));

        // Bucket by phase (Ord on SoftMutationPhase gives natural ordering)
        let mut by_phase: BTreeMap<SoftMutationPhase, Vec<mm_meta::soft_mutations::SoftMutation>> =
            BTreeMap::new();
        for sm in soft_mutations {
            by_phase.entry(sm.phase()).or_default().push(sm);
        }

        let mut phases: std::collections::VecDeque<_> = by_phase.into_iter().collect();

        // Queue first phase immediately, stash the rest
        if let Some((_phase, batch)) = phases.pop_front() {
            self.pending_soft_mutation_phases = phases;
            for sm in batch {
                self.enqueue_one(Task::SoftMutation(sm), label.clone());
            }
        }
    }

    // -------------------------------------------------------------------------
    // Maintenance Task Queueing (bypasses accepting_mutations gate)
    // -------------------------------------------------------------------------

    /// Queue a maintenance task for async execution on the rayon pool.
    ///
    /// Maintenance tasks bypass the `accepting_mutations` gate and can run
    /// before observing completes. Called only after operator approval.
    pub(super) fn queue_maintenance(&mut self, task: crate::meta::maintenance::DbMaintenanceTask) {
        self.transition_to_working();
        self.enqueue_one(Task::Maintenance(task), None);
    }
}
