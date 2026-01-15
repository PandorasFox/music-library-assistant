//! Application State Re-exports
//!
//! This module re-exports Eye types from the eye module for backwards compatibility.
//! The Eye animation system has been moved to `ui/eye.rs`.

// Re-export Eye types for backwards compatibility
pub use super::eye::{
    Eye as EyeAnimation,
    EyeFrame,
    EYE_OPEN,
    EYE_CLOSING,
    EYE_CLOSED,
};
