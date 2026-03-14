//! Inbox Corpus Match Resolution Modal
//!
//! Data wrapper, buttons, and actions live in `mm_ui::resolutions::inbox_corpus_match`.
//! This module provides the ratatui-specific `ModalFrame` impl.

mod preview;
pub mod types;

pub use types::{
    InboxCorpusMatchAction, InboxCorpusMatchData, InboxCorpusMatchModalData,
    InboxCorpusMatchState,
};
