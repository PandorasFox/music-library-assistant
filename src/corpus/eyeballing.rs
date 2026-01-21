//! Eyeballing Module
//!
//! Corpus state observation driven by the Eye. The eyeballing system detects
//! out-of-band changes to the corpus by comparing filesystem state to the
//! database index.
//!
//! ## Overview
//!
//! - **Startup**: Eyeballing ALWAYS runs before the Eye opens
//! - **Runtime**: Lazy eyeballing triggered by Eye blink rolls (roll of 13)
//!
//! ## Computation Chain
//!
//! ```text
//! WalkCorpus ──► CompareInodes ──► VerifyMtime ──► VerifyTags
//!                    │                   │              │
//!                    ▼                   ▼              ▼
//!             MissingFromDisk     (spawn next)   OutOfBandTagChange
//!             MissingFromIndex
//! ```
//!
//! Eyeballing is queued via `TaskDaemon::start_lazy_eyeball()` or
//! `TaskDaemon::start_paranoid_eyeball()`.
