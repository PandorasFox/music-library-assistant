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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProtocolBinding {
    /// Fire-and-forget command (no transaction).
    Command(CommandPayload),

    /// Query for data, construct mutations, submit as transaction.
    /// Mutation construction happens at runtime from query results.
    Transaction {
        decision_key: DecisionKey,
        label: String,
        data_query: Option<DataQuery>,
    },

    /// Navigation only — no protocol message.
    Navigation,
}

/// Which domain query to run to get data for mutation construction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DataQuery {
    /// GetIntakeConfirmation → IntakeConfirmationState
    IntakeConfirmation,
    /// GetDeployData → deploy plan
    DeployData,
}
