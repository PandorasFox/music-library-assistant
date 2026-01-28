//! Eyeballing Module
//!
//! Corpus state observation driven by the Eye. The eyeballing system detects
//! out-of-band changes to the corpus by comparing filesystem state to the
//! database index.
//!
//! ## Overview
//!
//! - **Startup**: Eyeballing ALWAYS runs before the Eye opens
//! - **Runtime**: Eyeballing triggered by Eye blink rolls (roll of 13)
//!
//! ## Computation Chain
//!
//! ```text
//! WalkCorpus ──► CompareInodes ──► VerifyMtime ──► VerifyTags
//!                    │                   │              │
//!                    ▼                   ▼              ▼
//!             MissingFile         (spawn next)   OutOfBandTagSync/Conflict
//!             UnindexedFile
//! ```
//!
//! Eyeballing is queued via `Witch::start_observing()`.
