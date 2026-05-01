//! Transaction lifecycle management for witnessed decisions.
//!
//! This module is part of the Witch subsystem. See `witch/mod.rs` for overview.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::db::types::Zone;
use crate::meta::decisions::{
    Decision, DecisionKey, DiscardSummary, PendingTransaction, TransactionError,
};
use crate::meta::mutations::dir_config_edit::{
    ApplyBatchDirConfigEditsMutation, DirConfigEditEntry,
};
use crate::meta::mutations::tag_edit::ApplyTagOpsMutation;
use crate::meta::mutations::{
    Mutation, MutationDispatch, MutationExecutionStage, MutationOrigin, TagOp,
};

/// Coalesce ApplyTagOps mutations into per-zone mutations.
///
/// Multiple decisions may generate overlapping tag operations for the same inode.
/// This function:
/// 1. Extracts all TagOps from ApplyTagOps mutations, grouped by zone
/// 2. Deduplicates by (inode, tag_name, old_value) → last new_value wins
/// 3. Returns one coalesced ApplyTagOps per zone, plus other mutations unchanged
///
/// This ensures that if two signals affect the same track, both fixes are applied
/// rather than the later one clobbering the earlier.
fn coalesce_tag_ops(mutations: Vec<Mutation>) -> Vec<Mutation> {
    let mut ops_by_zone: HashMap<Zone, Vec<TagOp>> = HashMap::new();
    let mut other: Vec<Mutation> = Vec::new();

    for mutation in mutations {
        match mutation {
            Mutation::ApplyTagOps(m) => ops_by_zone.entry(m.zone).or_default().extend(m.ops),
            m => other.push(m),
        }
    }

    if ops_by_zone.is_empty() {
        return other;
    }

    let mut result = Vec::new();

    for (zone, all_ops) in ops_by_zone {
        // Deduplicate overlapping ops for the same inode+tag.
        //
        // Replace/Drop ops (old_value is Some): keyed by (inode, tag_name, old_value)
        // so that if two decisions both want to replace the same source value,
        // the last one wins.
        //
        // Add ops (old_value is None): keyed by the FULL tuple including new_value,
        // since multiple adds for the same tag name are independent multi-value
        // inserts (e.g., GENRE="Rock" + GENRE="Metal" from a compound split).
        // Using old_value alone as part of the key would collapse them.
        let mut replace_drop: HashMap<(i64, String, Option<String>), Option<String>> =
            HashMap::new();
        let mut adds: HashSet<(i64, String, String)> = HashSet::new();

        for op in all_ops {
            if op.is_nop() {
                continue;
            }
            match (&op.old_value, &op.new_value) {
                (None, Some(new)) => {
                    // Add op: deduplicate by full (inode, tag_name, new_value)
                    adds.insert((op.inode, op.tag_name.clone(), new.clone()));
                }
                _ => {
                    // Replace or Drop: last writer wins per (inode, tag_name, old_value)
                    replace_drop.insert(
                        (op.inode, op.tag_name.clone(), op.old_value.clone()),
                        op.new_value,
                    );
                }
            }
        }

        let mut final_ops: Vec<TagOp> = replace_drop
            .into_iter()
            .map(|((inode, tag_name, old_value), new_value)| TagOp {
                inode,
                tag_name,
                old_value,
                new_value,
            })
            .collect();
        final_ops.extend(adds.into_iter().map(|(inode, tag_name, new_value)| TagOp {
            inode,
            tag_name,
            old_value: None,
            new_value: Some(new_value),
        }));

        if !final_ops.is_empty() {
            result.push(Mutation::ApplyTagOps(ApplyTagOpsMutation {
                ops: final_ops,
                zone,
            }));
        }
    }

    // Tag ops before other mutations
    result.extend(other);
    result
}
/// Coalesce ApplyDirConfigEdit mutations into a single batch write.
///
/// Multiple dir config edits targeting the same dirs.kdl file would race
/// when executed in parallel on rayon. This function:
/// 1. Extracts all ApplyDirConfigEdit mutations
/// 2. If there are 2+, merges them into a single ApplyBatchDirConfigEdits
/// 3. Builds a merged Config by overlaying each edit's changes
///
/// With only 0-1 dir config edits, returns mutations unchanged.
fn coalesce_dir_config_edits(mutations: Vec<Mutation>) -> Vec<Mutation> {
    let mut dir_edits: Vec<crate::meta::mutations::dir_config_edit::ApplyDirConfigEditMutation> =
        Vec::new();
    let mut other: Vec<Mutation> = Vec::new();

    for mutation in mutations {
        match mutation {
            Mutation::ApplyDirConfigEdit(m) => dir_edits.push(*m),
            m => other.push(m),
        }
    }

    if dir_edits.len() <= 1 {
        // Nothing to coalesce — put back as-is
        for m in dir_edits {
            other.push(Mutation::ApplyDirConfigEdit(Box::new(m)));
        }
        return other;
    }

    // Build merged new_config: start from first, overlay subsequent edits
    let mut merged_config = dir_edits[0].new_config.clone();
    for edit in &dir_edits[1..] {
        let mut found = false;
        for sd in &mut merged_config.source_dirs {
            if sd.path == edit.source_path {
                *sd = edit.new_dir.clone();
                found = true;
                break;
            }
        }
        if !found {
            merged_config.source_dirs.push(edit.new_dir.clone());
        }
    }
    // Elide default entries — they carry no information
    merged_config.source_dirs.retain(|sd| !sd.is_default());

    // Build batch entries
    let entries: Vec<DirConfigEditEntry> = dir_edits
        .into_iter()
        .map(|m| DirConfigEditEntry {
            source_path: m.source_path,
            old_dir: m.old_dir,
            new_dir: m.new_dir,
        })
        .collect();

    other.push(Mutation::ApplyBatchDirConfigEdits(Box::new(
        ApplyBatchDirConfigEditsMutation {
            edits: entries,
            new_config: merged_config,
        },
    )));
    other
}

impl super::Witch {
    // -------------------------------------------------------------------------
    // Transaction Helpers
    // -------------------------------------------------------------------------

    /// Require an active transaction, returning error if none exists.
    pub(super) fn require_active_transaction(
        &self,
        operation: &str,
    ) -> Result<(), TransactionError> {
        if self.pending_transaction.is_none() {
            crate::logging::log_mutation(format!(
                "[TRANSACTION] {} REJECTED: no active transaction",
                operation
            ));
            Err(TransactionError::NoActiveTransaction)
        } else {
            Ok(())
        }
    }

    // -------------------------------------------------------------------------
    // Transaction API
    // -------------------------------------------------------------------------

    /// Start a new transaction.
    ///
    /// Called by modals when the user is about to be presented with Decisions.
    /// Only one transaction may be active at a time.
    ///
    /// Returns Err if a transaction is already active.
    pub fn start_transaction(&mut self, label: &str) -> Result<(), TransactionError> {
        if self.pending_transaction.is_some() {
            crate::logging::log_mutation(format!(
                "[TRANSACTION] start_transaction({:?}) REJECTED: already active",
                label
            ));
            return Err(TransactionError::AlreadyActive);
        }

        crate::logging::log_mutation(format!("[TRANSACTION] start_transaction({:?}) OK", label));
        self.pending_transaction = Some(PendingTransaction::new(label));
        self.sync_handled_sources();
        Ok(())
    }

    /// Add a decision to the transaction.
    ///
    /// - `key`: Semantic key identifying the decision source and item
    /// - `decision`: A `Decision` (label + mutations, serializable)
    ///
    /// Overwrites any existing decision at the same key.
    /// Returns Err if no transaction is active.
    pub fn add_decision(
        &mut self,
        key: DecisionKey,
        decision: Decision,
    ) -> Result<(), TransactionError> {
        let mutation_count = decision.mutations.len();

        self.require_active_transaction(&format!(
            "add_decision(key={}, label={:?}, mutations={})",
            key, decision.label, mutation_count
        ))?;

        let txn = self.pending_transaction.as_mut().unwrap();

        crate::logging::log_mutation(format!(
            "[TRANSACTION] add_decision(key={}, label={:?}, mutations={}) OK - txn now has {} decisions",
            key, decision.label, mutation_count, txn.decision_count() + 1
        ));

        // Log each mutation for debugging
        for (i, m) in decision.mutations.iter().enumerate() {
            crate::logging::log_mutation(format!("[TRANSACTION]   mutation[{}]: {:?}", i, m));
        }

        txn.decisions.insert(key, decision);

        self.sync_handled_sources();
        Ok(())
    }

    /// Remove an entire decision from the active transaction.
    pub fn remove_decision(
        &mut self,
        key: &DecisionKey,
    ) -> Result<(), TransactionError> {
        self.require_active_transaction(&format!("remove_decision(key={})", key))?;

        let txn = self.pending_transaction.as_mut().unwrap();
        if txn.remove_decision(key).is_some() {
            crate::logging::log_mutation(format!(
                "[TRANSACTION] remove_decision(key={}) OK - txn now has {} decisions",
                key,
                txn.decision_count()
            ));
        }
        self.sync_handled_sources();
        Ok(())
    }

    /// Confirm the transaction - queue all mutations for execution.
    ///
    /// This is the primary way to add mutations to the execution queue.
    /// Requires a witness for the commit decision itself.
    ///
    /// Returns summary of what was committed, or error if mutations not accepted.
    pub fn confirm_transaction(
        &mut self,
    ) -> Result<(), TransactionError> {
        // Gate: mutations must be accepted (eye is Awake, not read-only)
        if !self.accepting_mutations() {
            crate::logging::log_mutation(
                "[TRANSACTION] confirm_transaction REJECTED: not accepting mutations (eyeballing incomplete or read-only mode)"
            );
            return Err(TransactionError::NotAcceptingMutations);
        }

        self.require_active_transaction("confirm_transaction")?;

        let txn = self.pending_transaction.take().unwrap();
        self.sync_handled_sources();

        let decision_count = txn.decision_count();
        let mut mutation_count = 0;

        // Collect decision labels (sorted for stable ordering) before consuming decisions
        let mut decision_labels: Vec<String> = txn
            .decisions
            .values()
            .map(|d| d.label.clone())
            .collect();
        decision_labels.sort();

        // Collect all mutations from all decisions, validating mutation kinds
        let raw_mutations: Vec<Mutation> = txn
            .decisions
            .into_iter()
            .flat_map(|(key, d)| {
                mutation_count += d.mutations.len();
                // Validate that each mutation's kind is allowed for this decision key
                #[cfg(debug_assertions)]
                {
                    let allowed = key.allowed_mutation_kinds();
                    for m in &d.mutations {
                        debug_assert!(
                            allowed.contains(&m.kind()),
                            "Decision {:?} contains disallowed mutation kind {:?} (mutation: {:?}). \
                             Allowed kinds: {:?}",
                            key, m.kind(), m, allowed
                        );
                    }
                }
                let _ = key; // suppress unused in release
                d.mutations
            })
            .collect();

        // Coalesce ApplyTagOps mutations to handle overlapping edits from multiple signals
        let all_mutations = coalesce_tag_ops(raw_mutations);
        // Coalesce ApplyDirConfigEdit mutations into a single atomic dirs.kdl write
        let all_mutations = coalesce_dir_config_edits(all_mutations);

        crate::logging::log_mutation(format!(
            "[TRANSACTION] confirm_transaction OK - {} decisions, {} mutations (after coalescing: {})",
            decision_count, mutation_count, all_mutations.len()
        ));

        if all_mutations.is_empty() {
            crate::logging::log_mutation(
                "[TRANSACTION] confirm_transaction: no mutations to queue (empty transaction)",
            );
            return Ok(());
        }

        // Bucket mutations by execution stage (BTreeMap gives ordered iteration via Ord)
        let mut by_stage: BTreeMap<MutationExecutionStage, Vec<Mutation>> = BTreeMap::new();
        for mutation in all_mutations {
            let executor = mutation.as_executor();
            debug_assert_eq!(
                executor.origin(),
                MutationOrigin::Staged,
                "ChainEmitted mutation {:?} found in transaction — \
                 these are only spawned during execution, never directly staged",
                mutation
            );
            let stage = executor.execution_stage();
            by_stage.entry(stage).or_default().push(mutation);
        }

        // Convert to ordered VecDeque of phases
        let mut phases: std::collections::VecDeque<(MutationExecutionStage, Vec<Mutation>)> =
            by_stage.into_iter().collect();

        crate::logging::log_mutation(format!(
            "[TRANSACTION] Staged execution: {} phase(s): {:?}",
            phases.len(),
            phases
                .iter()
                .map(|(s, m)| format!("{:?}({})", s, m.len()))
                .collect::<Vec<_>>()
        ));

        // Generate a timestamped session label for logging.
        let composite_label = if decision_labels.is_empty() {
            txn.label.clone()
        } else {
            decision_labels.join(" + ")
        };
        let session_label = format!(
            "{} @ {}",
            composite_label,
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
        );

        // Queue first phase immediately, stash remainder for drain-and-advance
        if let Some((stage, mutations)) = phases.pop_front() {
            crate::logging::log_mutation(format!(
                "[TRANSACTION] Queueing first phase: {:?} ({} mutations)",
                stage,
                mutations.len()
            ));
            self.pending_mutation_phases = phases;
            self.queue_mutations_internal(mutations, Some(session_label));
        }

        Ok(())
    }

    /// Discard the transaction - drop all accumulated decisions.
    ///
    /// Does not require a gesture — discarding is a safe, non-mutating operation.
    /// Any code that has access to the Witch can discard (cancel handlers, etc.).
    ///
    /// Returns summary of what was discarded.
    pub fn discard_transaction(&mut self) -> Result<DiscardSummary, TransactionError> {
        self.require_active_transaction("discard_transaction")?;

        let txn = self.pending_transaction.take().unwrap();
        self.sync_handled_sources();

        crate::logging::log_mutation(format!(
            "[TRANSACTION] discard_transaction OK - discarded {} decisions, {} mutations",
            txn.decision_count(),
            txn.mutation_count()
        ));

        Ok(DiscardSummary)
    }

    /// Bulk-approve N releases server-side, eliminating per-decision round-trips.
    ///
    /// Loads the review/staging data, builds per-release decisions via
    /// `build_release_approval_decisions`, and streams all decisions into
    /// the pending transaction via `add_decision` — opening a fresh
    /// transaction if none is active, otherwise appending to the existing
    /// one. Does NOT confirm — operator reviews and confirms separately.
    ///
    /// Appending (rather than discarding) is what lets the operator compose
    /// multiple approval batches — e.g. approve Perfect matches, then Full
    /// matches — without earlier approvals being clobbered by later calls.
    /// Re-approving the same release within one transaction is idempotent
    /// since `add_decision` keys on `MbReleaseApproval { release_id }`.
    ///
    /// Returns the full `ApprovalSummary` (staged + skipped at both release
    /// and track granularity) so callers can render an unambiguous status.
    pub fn batch_approve_releases(
        &mut self,
        release_ids: Vec<String>,
    ) -> Result<mm_meta::external::approval::ApprovalSummary, TransactionError> {
        if release_ids.is_empty() {
            return Err(TransactionError::Other(
                "no release IDs supplied".to_string(),
            ));
        }

        // 1. Snapshot config (lock-free).
        let cfg = self
            .read_config(|c| {
                (
                    c.opinions.external_matching.preferred_locales.clone(),
                    c.opinions.external_matching.credit_routing.clone(),
                    c.opinions.external_matching.mb_tag_names.clone(),
                )
            })
            .ok_or_else(|| {
                TransactionError::Other("config not yet initialized".to_string())
            })?;
        let (locales, routing, tag_names) = cfg;

        // 2. Open a fresh read-only DB connection for the load phase.
        let db_path = crate::config::get_db_path()
            .map_err(|e| TransactionError::Other(format!("get_db_path: {e}")))?;
        let db = crate::db::Database::open_read_only(&db_path)
            .map_err(|e| TransactionError::Other(format!("open_read_only: {e}")))?;
        let read_db = crate::db::ReadOnlyDb::new(&db);

        // 3. Load the review set and filter to requested IDs.
        //    `load_release_review` returns all reviewable releases; we
        //    filter in-process. Future optimization: thread an
        //    Option<&[String]> through the loader to query only the
        //    requested IDs.
        use crate::meta::views::external_matches::{
            ApprovalTrackInput, ReleaseApprovalInput, ReleaseReviewFilter,
        };
        let review = crate::db::domain::load_release_review(
            ReleaseReviewFilter::All,
            &read_db,
        );
        let requested: HashSet<&str> =
            release_ids.iter().map(String::as_str).collect();
        let approval_inputs: Vec<ReleaseApprovalInput> = review
            .releases
            .iter()
            .filter(|r| requested.contains(r.release_id.as_str()))
            .map(|r| ReleaseApprovalInput {
                release_id: r.release_id.clone(),
                tracks: r
                    .tracks
                    .iter()
                    .filter_map(|t| {
                        let inode = t.matched_inode?;
                        Some(ApprovalTrackInput {
                            inode,
                            recording_id: t.recording_id.clone(),
                            track_title: t.mb_title.clone(),
                            track_position: t.position as u32,
                            medium_position: t.medium_position,
                        })
                    })
                    .collect(),
            })
            .filter(|r| !r.tracks.is_empty())
            .collect();
        if approval_inputs.is_empty() {
            return Err(TransactionError::Other(
                "no matched tracks for selected releases".to_string(),
            ));
        }

        // 4. Collect unique IDs needed for staging.
        let mut all_release_ids: Vec<String> = Vec::new();
        let mut all_recording_ids: Vec<String> = Vec::new();
        let mut all_inodes: Vec<i64> = Vec::new();
        let mut seen_r: HashSet<String> = HashSet::new();
        let mut seen_rec: HashSet<String> = HashSet::new();
        for rd in &approval_inputs {
            if seen_r.insert(rd.release_id.clone()) {
                all_release_ids.push(rd.release_id.clone());
            }
            for t in &rd.tracks {
                all_inodes.push(t.inode);
                if seen_rec.insert(t.recording_id.clone()) {
                    all_recording_ids.push(t.recording_id.clone());
                }
            }
        }

        // 5. Load MB cache + current tags (chunked-batch staging loader).
        let staging = crate::db::domain::load_release_staging_data(
            &read_db,
            &all_release_ids,
            &all_recording_ids,
            &all_inodes,
        );

        crate::logging::log_general(format!(
            "[BATCH_APPROVE] staging loaded: {} releases, {} recordings, {} artists, {} inode_tags",
            staging.bundle.releases.len(),
            staging.bundle.recordings.len(),
            staging.bundle.artists.len(),
            staging.inode_tags.len(),
        ));

        // 6. Build decisions (pure transformation, no DB).
        let (decisions, summary) =
            mm_meta::external::approval::build_release_approval_decisions(
                &approval_inputs,
                &staging.bundle,
                &staging.inode_tags,
                &locales,
                &routing,
                &tag_names,
            );
        if decisions.is_empty() {
            return Err(TransactionError::Other(format!(
                "no tag operations produced (skipped {} across {} — \
                 likely missing recording data in MB cache)",
                mm_utils::count_noun(summary.skipped_tracks, "track"),
                mm_utils::count_noun(summary.skipped_releases, "release"),
            )));
        }

        // Drop the read-only DB handle before we start mutating; nothing
        // below needs it, and the read snapshot becomes stale once the
        // pending transaction confirms.
        drop(read_db);
        drop(db);

        // 7. Ensure a transaction exists, then stream decisions into it.
        //    If a transaction is already open (e.g. operator is composing
        //    Perfect + Full approvals), append to it; otherwise start a
        //    fresh one. add_decision is keyed on MbReleaseApproval so
        //    re-approving the same release is idempotent.
        let appending = self.pending_transaction.is_some();
        if !appending {
            let label = format!(
                "Approve {}",
                mm_utils::count_noun(summary.staged_releases, "release"),
            );
            self.start_transaction(&label)?;
        }

        crate::logging::log_general(format!(
            "[BATCH_APPROVE] {} {} releases [{} tracks] into transaction \
             (skipped {} tracks across {} releases)",
            if appending { "appending" } else { "streaming" },
            summary.staged_releases,
            summary.staged_tracks,
            summary.skipped_tracks,
            summary.skipped_releases,
        ));

        for ad in decisions {
            let key = DecisionKey::MbReleaseApproval {
                release_id: ad.release_id,
            };
            let mutations: Vec<Mutation> = ad
                .per_inode_ops
                .into_iter()
                .map(|ops| {
                    Mutation::ApplyTagOps(ApplyTagOpsMutation {
                        ops,
                        zone: Zone::Corpus,
                    })
                })
                .collect();
            self.add_decision(
                key,
                Decision {
                    label: ad.label,
                    mutations,
                },
            )?;
        }

        Ok(summary)
    }

    /// Batch-apply VariousArtistsOverride suggestions. For each application,
    /// fans out to the inodes packed to that release (per `signal_release_packing`)
    /// and stages an `ApplyTagOps` decision rewriting `ALBUMARTIST`. Mirrors
    /// `batch_approve_releases` — opens a fresh transaction if none active,
    /// appends to the pending one otherwise. Decisions are keyed on
    /// `VaOverrideApplication { release_id }` for idempotency.
    pub fn batch_apply_va_overrides(
        &mut self,
        applications: Vec<mm_meta::protocol::VaOverrideApplication>,
    ) -> Result<mm_meta::external::va_override::VaOverrideSummary, TransactionError> {
        use mm_meta::external::va_override::{build_va_override_decisions, VaOverrideInput};

        if applications.is_empty() {
            return Err(TransactionError::Other(
                "no VA-override applications supplied".to_string(),
            ));
        }

        let db_path = crate::config::get_db_path()
            .map_err(|e| TransactionError::Other(format!("get_db_path: {e}")))?;
        let db = crate::db::Database::open_read_only(&db_path)
            .map_err(|e| TransactionError::Other(format!("open_read_only: {e}")))?;
        let read_db = crate::db::ReadOnlyDb::new(&db);

        // Fan out each release_id to its packed inodes.
        let mut inputs: Vec<VaOverrideInput> = Vec::with_capacity(applications.len());
        let mut all_inodes: Vec<i64> = Vec::new();
        let mut skipped_no_inodes = 0usize;
        for app in applications {
            let inodes = read_db
                .get_inodes_for_packed_release(&app.release_id)
                .unwrap_or_default();
            if inodes.is_empty() {
                skipped_no_inodes += 1;
                continue;
            }
            all_inodes.extend(inodes.iter().copied());
            inputs.push(VaOverrideInput {
                release_id: app.release_id,
                albumartist: app.albumartist,
                inodes,
            });
        }

        if inputs.is_empty() {
            return Err(TransactionError::Other(format!(
                "no packed inodes found for any of the {} applications",
                skipped_no_inodes
            )));
        }

        // Reuse the existing staging loader for batched current-tag fetch.
        // We don't need MB cache, just inode_tags — pass empty release/recording
        // ID slices so the bundle ends up empty (cheap).
        let staging = crate::db::domain::load_release_staging_data(
            &read_db,
            &[],
            &[],
            &all_inodes,
        );

        let (decisions, mut summary) =
            build_va_override_decisions(&inputs, &staging.inode_tags);

        // Add releases skipped at fan-out time to the summary so the operator
        // sees them in the response.
        summary.skipped_releases += skipped_no_inodes;

        if decisions.is_empty() {
            return Err(TransactionError::Other(format!(
                "no tag operations produced (every selected release was \
                 already at the chosen ALBUMARTIST or had no packed inodes; \
                 already_matching={}, skipped={})",
                summary.already_matching_inodes, summary.skipped_releases,
            )));
        }

        drop(read_db);
        drop(db);

        let appending = self.pending_transaction.is_some();
        if !appending {
            let label = format!(
                "Apply {}",
                mm_utils::count_noun(summary.staged_releases, "VA override"),
            );
            self.start_transaction(&label)?;
        }

        crate::logging::log_general(format!(
            "[BATCH_VA_OVERRIDE] {} {} VA overrides [{} inodes] into transaction \
             (already_matching={}, skipped_releases={})",
            if appending { "appending" } else { "streaming" },
            summary.staged_releases,
            summary.staged_inodes,
            summary.already_matching_inodes,
            summary.skipped_releases,
        ));

        for ad in decisions {
            let key = DecisionKey::VaOverrideApplication {
                release_id: ad.release_id,
            };
            let mutations: Vec<Mutation> = ad
                .per_inode_ops
                .into_iter()
                .map(|ops| {
                    Mutation::ApplyTagOps(ApplyTagOpsMutation {
                        ops,
                        zone: Zone::Corpus,
                    })
                })
                .collect();
            self.add_decision(
                key,
                Decision {
                    label: ad.label,
                    mutations,
                },
            )?;
        }

        Ok(summary)
    }
}
