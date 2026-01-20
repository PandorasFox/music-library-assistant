//! Corpus Module
//!
//! Unified API for corpus indexing, health, analysis, and reporting. This module
//! is the core domain for understanding and maintaining the audio corpus.
//!
//! ## Submodules
//!
//! - `db/` - Database layer (types, queries)
//! - `health/` - Health issue detection, filtering, and library health
//! - `mutations/` - Standardized mutation interface for all corpus changes
//! - `computations/` - Read-only operations that derive facts (tag verification, etc.)
//! - `eyeballing/` - Corpus state observation driven by the Eye

pub mod computations;
pub mod db;
pub mod deploy;
pub mod eyeballing;
pub mod health;
pub mod metadata;
pub mod mutations;



