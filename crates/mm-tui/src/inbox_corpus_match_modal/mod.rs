//! Inbox Corpus Match Resolution Modal
//!
//! Provides the interactive workflow for resolving inbox files that have
//! fingerprint matches against corpus files. Shows quality comparison and
//! allows the operator to stash equivalent/inferior inbox copies.

pub mod preview;
pub mod types;

pub use preview::{InboxCorpusMatchPreviewAction, InboxCorpusMatchPreviewState};
pub use types::InboxCorpusMatchModalData;
