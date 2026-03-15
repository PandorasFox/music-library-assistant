//! ProtocolBinding: typed button→Witch mapping.
//!
//! Encodes what protocol action a UI element triggers when confirmed.
//! Both TUI action handlers and web onclick generators consume this to
//! ensure the right protocol message is sent.

use serde::{Deserialize, Serialize};

use mm_meta::decisions::DecisionKey;
use mm_meta::protocol::CommandPayload;

/// What protocol action a UI element triggers when confirmed.
///
/// Encodes the button→Witch mapping at the type level. Both TUI action
/// handlers and web onclick generators consume this to ensure the right
/// protocol message is sent.
///
/// `Transaction` is display metadata only — the client builds mutations
/// locally and submits via `/tx/start` → `/tx/add` → `/tx/confirm`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProtocolBinding {
    /// Fire-and-forget command (no transaction).
    Command(CommandPayload),

    /// Transaction display info. The client builds `Vec<Mutation>` locally
    /// and submits via the transaction API. `decision_key` and `label`
    /// identify the decision for review display.
    Transaction {
        decision_key: DecisionKey,
        label: String,
    },

    /// Navigation only — no protocol message.
    Navigation,
}

impl ProtocolBinding {
    /// Extract the `DecisionKey` from a `Transaction` binding.
    ///
    /// Returns `None` for `Command` and `Navigation` bindings.
    pub fn decision_key(&self) -> Option<&DecisionKey> {
        match self {
            Self::Transaction { decision_key, .. } => Some(decision_key),
            _ => None,
        }
    }
}
