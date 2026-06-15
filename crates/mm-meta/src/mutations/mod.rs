//! Mutation data types.
//!
//! Struct definitions for all corpus mutations. These are pure data types
//! (Serialize/Deserialize) with no execution logic. The `MutationExecutor`
//! trait and impls live in the `mm` crate.

pub mod builders;
pub mod config_edit;
pub mod diffable;
pub mod dir_config_edit;
pub mod file_ops;
pub mod genre_vocabulary;
pub mod indexing;
pub mod tag_edit;
pub mod transcode;
mod types;

pub use types::*;
