//! Signal data types — pure serializable payloads for signal BLOBs.
//!
//! Signal wrapper structs (with `SignalContentHash` and store impls) stay in mm.
//! This module contains only the inner data types that are serialized as bincode
//! BLOBs within signal tables, plus pure enums used across the protocol boundary.

pub mod data;
pub mod packing_category;
pub use data::*;
pub use packing_category::PackingCategory;
