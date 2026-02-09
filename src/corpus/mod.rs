//! Corpus Module
//!
//! Unified API for corpus indexing, health, analysis, and reporting. This module
//! is the core domain for understanding and maintaining the audio corpus.
//!
//! ## Submodules
//!
//! - `db/` - Database layer (types, queries)
//! - `health/` - Health issue detection, filtering, and library health
//!
//! ## Moved to `meta/`
//!
//! - `meta::mutations/` - Standardized mutation interface for all corpus changes
//! - `meta::computations/` - Read-only operations that derive facts
//! - `meta::signals/` - Signal types (formerly in `corpus::db::types`)

pub mod codecs;
pub mod db;
pub mod deploy;
pub mod health;
pub mod metadata;
pub mod paths;
pub mod tags;
pub mod transcode;



