//! Meta Module - Core inter-system abstractions.
//!
//! Contains the first-class concepts that span MM's subsystems:
//! - `signals/` - Health signal types and structures
//! - `mutations/` - Corpus mutation types and executors
//! - `computations/` - Background computation types and executors

pub mod signals;
pub mod mutations;
pub mod computations;
