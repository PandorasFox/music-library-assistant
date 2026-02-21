//! Meta Module - Core inter-system abstractions.
//!
//! Contains the first-class concepts that span MM's subsystems:
//! - `views` - Inter-system aggregate types (InsightsData, DeployStatus, etc.)
//! - `decisions/` - Operator decision and transaction data types
//! - `signals/` - Health signal types and structures
//! - `mutations/` - Corpus mutation types and executors
//! - `computations/` - Background computation types and executors
//! - `maintenance/` - Database maintenance tasks (migrations, vacuum)

pub mod views;
pub mod decisions;
pub mod signals;
pub mod mutations;
pub mod computations;
pub mod maintenance;
pub mod recomputation;
pub mod external;
