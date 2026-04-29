//! Re-export of the shared release approval builder.
//!
//! The implementation lives in `mm_meta::external::approval` so the witch
//! can call it without taking a dep on mm-ui. This module remains as a
//! re-export so existing TUI/web import paths keep working.

pub use mm_meta::external::approval::build_release_approval_decisions;
