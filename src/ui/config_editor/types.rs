//! Config Editor Types
//!
//! Typed representations of config fields, groups, and edit state.

/// Tracks where a field value came from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FieldSource {
    /// Value matches the compiled default.
    Default,
    /// Value was loaded from config.kdl.
    Loaded,
    /// Value was edited this session.
    Edited,
}

/// Typed config value — avoids string round-tripping for edits.
#[derive(Clone, Debug)]
pub enum ConfigValue {
    Bool(bool),
    Float(f64),
    Uint(usize),
    UintU32(u32),
    SignedInt(i64),
    OptionalUint(Option<usize>),
    String(String),
    StringList(Vec<String>),
    Enum {
        selected: usize,
        options: Vec<&'static str>,
    },
    /// Duration in seconds, edited/displayed as humantime strings (e.g. "3m", "180s").
    Duration(u64),
    /// Set of strings (opens StringSetEditor popup).
    StringSet(Vec<String>),
    /// Map of string pairs (opens StringPairMapEditor popup).
    StringPairMap(Vec<(String, String)>),
    /// Map of string to list of strings (opens StringListMapEditor popup).
    StringListMap(Vec<(String, Vec<String>)>),
}

impl ConfigValue {
    /// Render value as a display string.
    pub fn display(&self) -> String {
        match self {
            ConfigValue::Bool(b) => if *b { "true" } else { "false" }.to_string(),
            ConfigValue::Float(f) => format!("{}", f),
            ConfigValue::Uint(n) => format!("{}", n),
            ConfigValue::UintU32(n) => format!("{}", n),
            ConfigValue::SignedInt(n) => format!("{}", n),
            ConfigValue::OptionalUint(None) => "auto".to_string(),
            ConfigValue::OptionalUint(Some(n)) => format!("{}", n),
            ConfigValue::Duration(secs) => {
                if *secs == 0 {
                    "disabled".to_string()
                } else {
                    humantime::format_duration(std::time::Duration::from_secs(*secs)).to_string()
                }
            }
            ConfigValue::String(s) => {
                if s.is_empty() { "(empty)".to_string() } else { s.clone() }
            }
            ConfigValue::StringList(v) => v.join(", "),
            ConfigValue::Enum { selected, options } => {
                options.get(*selected).unwrap_or(&"?").to_string()
            }
            ConfigValue::StringSet(v) => {
                if v.is_empty() {
                    "(empty)".to_string()
                } else {
                    format!("{} items", v.len())
                }
            }
            ConfigValue::StringPairMap(v) => {
                if v.is_empty() {
                    "(empty)".to_string()
                } else {
                    format!("{} pairs", v.len())
                }
            }
            ConfigValue::StringListMap(v) => {
                if v.is_empty() {
                    "(empty)".to_string()
                } else {
                    format!("{} tags", v.len())
                }
            }
        }
    }

    /// Check structural equality (same variant and value).
    pub fn eq_value(&self, other: &ConfigValue) -> bool {
        match (self, other) {
            (ConfigValue::Bool(a), ConfigValue::Bool(b)) => a == b,
            (ConfigValue::Float(a), ConfigValue::Float(b)) => (a - b).abs() < f64::EPSILON,
            (ConfigValue::Uint(a), ConfigValue::Uint(b)) => a == b,
            (ConfigValue::UintU32(a), ConfigValue::UintU32(b)) => a == b,
            (ConfigValue::SignedInt(a), ConfigValue::SignedInt(b)) => a == b,
            (ConfigValue::OptionalUint(a), ConfigValue::OptionalUint(b)) => a == b,
            (ConfigValue::Duration(a), ConfigValue::Duration(b)) => a == b,
            (ConfigValue::String(a), ConfigValue::String(b)) => a == b,
            (ConfigValue::StringList(a), ConfigValue::StringList(b)) => a == b,
            (ConfigValue::Enum { selected: a, .. }, ConfigValue::Enum { selected: b, .. }) => a == b,
            (ConfigValue::StringSet(a), ConfigValue::StringSet(b)) => a == b,
            (ConfigValue::StringPairMap(a), ConfigValue::StringPairMap(b)) => a == b,
            (ConfigValue::StringListMap(a), ConfigValue::StringListMap(b)) => a == b,
            _ => false,
        }
    }
}

/// A single editable field in the config editor.
pub struct ConfigField {
    pub label: &'static str,
    pub description: &'static str,
    pub value: ConfigValue,
    pub source: FieldSource,
    /// Value when the editor was opened (for NOP detection and reset).
    pub original_value: ConfigValue,
    /// Source when the editor was opened (restored on NOP or reset).
    pub original_source: FieldSource,
    pub restart_required: bool,
}

/// A group of related config fields (maps to a KDL block).
pub struct ConfigGroup {
    pub name: &'static str,
    pub fields: Vec<ConfigField>,
    pub collapsed: bool,
}

impl ConfigGroup {
    /// Count visible (non-collapsed) fields in this group.
    pub fn visible_field_count(&self) -> usize {
        if self.collapsed { 0 } else { self.fields.len() }
    }
}
