//! Corpus Module
//!
//! Pure audio domain: codecs, tags, paths, metadata, transcode, deploy, health.
//!
//! ## Submodules
//!
//! - `health/` - Health issue detection, filtering, and library health
//!
//! ## Moved to top-level `db/`
//!
//! - `db/` - Database layer (types, queries, write thread)
//!
//! ## Moved to `meta/`
//!
//! - `meta::mutations/` - Standardized mutation interface for all corpus changes
//! - `meta::computations/` - Read-only operations that derive facts
//! - `meta::signals/` - Signal types

pub mod codecs;
pub mod deploy;
pub mod health;
pub mod image_hash;
pub mod metadata;
pub mod paths;
pub mod tags;
pub mod transcode;
