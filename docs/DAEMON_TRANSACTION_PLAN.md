# Daemon Transaction Interface Plan

This document specifies the transaction interface for the TaskDaemon. Transactions are the mechanism by which operator Decisions accumulate and eventually become queued Mutations.

## Design Goals

1. **Single entry point for mutations** - After this design, confirming a transaction is the ONLY way to add Mutations to the daemon execution queue
2. **Witnessed decisions** - Every Decision stored requires a DecisionWitness
3. **Ephemeral state** - Transactions are NOT persisted to disk; they are in-memory accumulation
4. **Clean containment** - Elegant solution to safely prepare state changes before commitment

## Non-Goals

- Persistence/durability (transactions are ephemeral)
- Affecting Computations (they don't require Decisions)
- Complex indexing (UI provides simple int indices)

---

## Data Structures

```rust
// In daemon.rs or daemon/transaction.rs

/// A witnessed decision with its associated pending mutations
#[derive(Debug, Clone)]
pub struct WitnessedDecision {
    /// The witness proving operator confirmation
    pub witness: DecisionWitness,

    /// Human-readable label for this decision
    pub label: String,

    /// The mutations this decision will produce when committed
    pub mutations: Vec<Mutation>,
}

/// An active transaction accumulating decisions
#[derive(Debug)]
pub struct PendingTransaction {
    /// Human-readable label for this transaction
    pub label: String,

    /// When the transaction was started
    pub started_at: Instant,

    /// Accumulated decisions by index
    /// Indexes are UI-provided, may have gaps, largely sequential
    decisions: HashMap<usize, WitnessedDecision>,
}

impl PendingTransaction {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            started_at: Instant::now(),
            decisions: HashMap::new(),
        }
    }

    /// Count of stored decisions
    pub fn decision_count(&self) -> usize {
        self.decisions.len()
    }

    /// Total mutations across all decisions
    pub fn mutation_count(&self) -> usize {
        self.decisions.values()
            .map(|d| d.mutations.len())
            .sum()
    }

    /// Get all decision indices (for iteration)
    pub fn indices(&self) -> Vec<usize> {
        let mut indices: Vec<_> = self.decisions.keys().copied().collect();
        indices.sort();
        indices
    }
}
```

---

## Transaction API

```rust
impl TaskDaemon {
    // ═══════════════════════════════════════════════════════════════
    // Transaction Lifecycle
    // ═══════════════════════════════════════════════════════════════

    /// Start a new transaction.
    ///
    /// Called by modals when the user is about to be presented with Decisions.
    /// Only one transaction may be active at a time.
    ///
    /// Returns Err if a transaction is already active.
    pub fn start_transaction(&mut self, label: &str) -> Result<(), TransactionError> {
        if self.pending_transaction.is_some() {
            return Err(TransactionError::AlreadyActive);
        }

        self.pending_transaction = Some(PendingTransaction::new(label));
        Ok(())
    }

    /// Check if a transaction is currently active.
    pub fn has_transaction(&self) -> bool {
        self.pending_transaction.is_some()
    }

    /// Get transaction info for UI display.
    pub fn transaction_info(&self) -> Option<TransactionInfo> {
        self.pending_transaction.as_ref().map(|txn| TransactionInfo {
            label: txn.label.clone(),
            decision_count: txn.decision_count(),
            mutation_count: txn.mutation_count(),
            started_at: txn.started_at,
        })
    }

    // ═══════════════════════════════════════════════════════════════
    // Decision Management
    // ═══════════════════════════════════════════════════════════════

    /// Add a witnessed decision to the transaction.
    ///
    /// - `idx`: UI-provided index (may have gaps, largely sequential)
    /// - `witness`: Proof of operator confirmation
    /// - `label`: Human-readable description
    /// - `mutations`: The mutations this decision represents
    ///
    /// Overwrites any existing decision at the same index.
    /// Returns Err if no transaction is active.
    pub fn add_decision(
        &mut self,
        idx: usize,
        witness: DecisionWitness,
        label: impl Into<String>,
        mutations: Vec<Mutation>,
    ) -> Result<(), TransactionError> {
        let txn = self.pending_transaction.as_mut()
            .ok_or(TransactionError::NoActiveTransaction)?;

        txn.decisions.insert(idx, WitnessedDecision {
            witness,
            label: label.into(),
            mutations,
        });

        Ok(())
    }

    /// Discard a single decision by index.
    ///
    /// Requires a witness - discarding is also a decision.
    /// Used primarily for review screen before confirm/discard.
    ///
    /// Returns the discarded decision, or None if no decision at that index.
    pub fn discard_decision(
        &mut self,
        idx: usize,
        _witness: DecisionWitness,
    ) -> Result<Option<WitnessedDecision>, TransactionError> {
        let txn = self.pending_transaction.as_mut()
            .ok_or(TransactionError::NoActiveTransaction)?;

        Ok(txn.decisions.remove(&idx))
    }

    /// Fetch a decision by index.
    ///
    /// Returns None if no decision stored at that index.
    pub fn get_decision(&self, idx: usize) -> Option<&WitnessedDecision> {
        self.pending_transaction.as_ref()
            .and_then(|txn| txn.decisions.get(&idx))
    }

    /// List all decision indices in the current transaction.
    pub fn decision_indices(&self) -> Vec<usize> {
        self.pending_transaction.as_ref()
            .map(|txn| txn.indices())
            .unwrap_or_default()
    }

    // ═══════════════════════════════════════════════════════════════
    // Transaction Resolution
    // ═══════════════════════════════════════════════════════════════

    /// Confirm the transaction - queue all mutations for execution.
    ///
    /// This is the ONLY way to add mutations to the execution queue.
    /// Requires a witness for the commit decision itself.
    ///
    /// Returns summary of what was committed.
    pub fn confirm_transaction(
        &mut self,
        _witness: DecisionWitness,
    ) -> Result<CommitSummary, TransactionError> {
        let txn = self.pending_transaction.take()
            .ok_or(TransactionError::NoActiveTransaction)?;

        let decision_count = txn.decision_count();
        let mut mutation_count = 0;

        // Collect all mutations from all decisions
        let all_mutations: Vec<Mutation> = txn.decisions
            .into_values()
            .flat_map(|d| {
                mutation_count += d.mutations.len();
                d.mutations
            })
            .collect();

        // Queue mutations for execution
        if !all_mutations.is_empty() {
            self.queue_mutations_internal(all_mutations, Some(txn.label));
        }

        Ok(CommitSummary {
            decision_count,
            mutation_count,
        })
    }

    /// Discard the transaction - drop all accumulated decisions.
    ///
    /// Requires a witness - discarding is also a decision.
    ///
    /// Returns summary of what was discarded.
    pub fn discard_transaction(
        &mut self,
        _witness: DecisionWitness,
    ) -> Result<DiscardSummary, TransactionError> {
        let txn = self.pending_transaction.take()
            .ok_or(TransactionError::NoActiveTransaction)?;

        Ok(DiscardSummary {
            decision_count: txn.decision_count(),
            mutation_count: txn.mutation_count(),
        })
    }
}

// ═══════════════════════════════════════════════════════════════
// Error and Summary Types
// ═══════════════════════════════════════════════════════════════

#[derive(Debug)]
pub enum TransactionError {
    /// Attempted to start a transaction when one is already active
    AlreadyActive,
    /// Attempted an operation requiring an active transaction
    NoActiveTransaction,
}

#[derive(Debug, Clone)]
pub struct TransactionInfo {
    pub label: String,
    pub decision_count: usize,
    pub mutation_count: usize,
    pub started_at: Instant,
}

#[derive(Debug, Clone)]
pub struct CommitSummary {
    pub decision_count: usize,
    pub mutation_count: usize,
}

#[derive(Debug, Clone)]
pub struct DiscardSummary {
    pub decision_count: usize,
    pub mutation_count: usize,
}
```

---

## Usage Patterns

### Starting a Flow

```rust
// Modal initialization
fn start_tag_edit_flow(daemon: &mut TaskDaemon, signals: &[Signal]) {
    daemon.start_transaction("Tag edits")
        .expect("No transaction should be active");

    // ... set up ViewContext, present first decision group
}
```

### Adding Decisions

```rust
// In modal input handler, when user confirms a decision
fn handle_confirm(&mut self, daemon: &mut TaskDaemon) -> ModalAction {
    let witness = confirm_decision();  // At keypress
    let mutations = self.collect_mutations();

    daemon.add_decision(
        self.group_idx,
        witness,
        format!("Edit tags for group {}", self.group_idx),
        mutations,
    ).expect("Transaction should be active");

    ModalAction::NextGroup
}
```

### Navigating Back

```rust
// When user navigates to a previous group
fn reload_group(&mut self, daemon: &TaskDaemon, group_idx: usize) {
    // Check if we already have a decision for this group
    if let Some(decision) = daemon.get_decision(group_idx) {
        // Restore edit state from the stored mutations
        self.restore_from_mutations(&decision.mutations);
    } else {
        // Fresh state from signals
        self.reset_to_signals();
    }
}
```

### Review Screen

```rust
// Review all decisions before commit
fn render_review(&self, daemon: &TaskDaemon) {
    let indices = daemon.decision_indices();
    for idx in indices {
        if let Some(decision) = daemon.get_decision(idx) {
            // Render decision summary
            println!("{}: {} mutations", decision.label, decision.mutations.len());
        }
    }
}

// User removes a decision from review
fn remove_decision(&mut self, daemon: &mut TaskDaemon, idx: usize) {
    let witness = confirm_decision();
    daemon.discard_decision(idx, witness);
}
```

### Committing

```rust
// User confirms entire transaction
fn handle_commit(&mut self, daemon: &mut TaskDaemon) {
    let witness = confirm_decision();

    match daemon.confirm_transaction(witness) {
        Ok(summary) => {
            println!("Committed {} decisions ({} mutations)",
                summary.decision_count, summary.mutation_count);
        }
        Err(e) => {
            println!("Commit failed: {:?}", e);
        }
    }
}
```

---

## Invariants

1. **Only one transaction active** - `start_transaction` fails if one exists
2. **All decisions witnessed** - No way to add unwitnessed decisions
3. **Mutations only via commit** - `confirm_transaction` is the sole entry point
4. **Computations unaffected** - They don't flow through transactions
5. **Ephemeral only** - No persistence, no recovery

---

## Implementation Notes

### Daemon Fields

```rust
pub struct TaskDaemon {
    // ... existing fields ...

    /// Active transaction (at most one)
    pending_transaction: Option<PendingTransaction>,
}
```

### Migration

1. Remove any direct `queue_mutation` public methods (keep internal)
2. Existing flows that queue mutations must go through transactions
3. Computations continue to use their existing paths (no change)
