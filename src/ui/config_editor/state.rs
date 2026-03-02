//! Config Editor State and Key Handling
//!
//! Manages cursor navigation, field editing, and the Save/Discard flow.

use crate::config::Config;
use crate::ui::input::InputAction;
use crate::ui::widgets::TextInputState;
use super::build;
use super::types::*;

/// Focus region within the config editor.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EditorFocus {
    Fields,
    Buttons,
}

/// Which button is highlighted when focus is on buttons.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EditorButton {
    Save,
    Discard,
}

/// Action produced by `handle_input`, consumed by the action handler.
pub enum ConfigEditorAction {
    None,
    Save,
    Discard,
    /// Cycle to next lateral view (Tab).
    CycleNext,
    /// Cycle to previous lateral view (Shift-Tab).
    CyclePrev,
}

/// Direction for deferred lateral ring navigation.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CycleDirection {
    Next,
    Prev,
}

/// Position within a collection field (for inline editing).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CollectionPosition {
    /// On a specific item within the collection.
    Item(usize),
    /// On the "add new" row.
    AddNew,
}

/// Full state for the config editor view.
pub struct ConfigEditorState {
    pub groups: Vec<ConfigGroup>,
    /// Snapshot of config when editor opened (for diffing and apply base).
    pub original_config: Config,
    /// Raw KDL text from config file (for source detection).
    pub original_kdl: Option<String>,
    /// Flat cursor position across all visible (non-collapsed) fields.
    pub cursor: usize,
    pub scroll_offset: usize,
    /// Active text editing state (None = navigating).
    pub text_input: Option<TextInputState>,
    /// Position within expanded collection field (None = not in collection).
    pub collection_pos: Option<CollectionPosition>,
    /// Position within a StringListMap item's separator sub-list (None = not in sub-list).
    pub sub_collection_pos: Option<CollectionPosition>,
    /// Focus: Fields vs Buttons.
    pub focus: EditorFocus,
    pub selected_button: EditorButton,
    /// When set, Discard navigates laterally instead of returning to Insights.
    /// Set by Tab/Shift-Tab when there are unsaved edits.
    pub pending_cycle: Option<CycleDirection>,
}

impl ConfigEditorState {
    /// Create a new config editor from the current config.
    pub fn new(config: &Config, kdl_content: Option<String>) -> Self {
        let groups = build::build_groups_from_config(config, kdl_content.as_deref());
        Self {
            groups,
            original_config: config.clone(),
            original_kdl: kdl_content,
            cursor: 0,
            scroll_offset: 0,
            text_input: None,
            collection_pos: None,
            sub_collection_pos: None,
            focus: EditorFocus::Fields,
            selected_button: EditorButton::Save,
            pending_cycle: None,
        }
    }

    /// Check if any field has been edited.
    pub fn has_edits(&self) -> bool {
        self.groups.iter().any(|g| g.fields.iter().any(|f| f.source == FieldSource::Edited))
    }

    /// Total number of visible (non-collapsed) fields across all groups.
    fn visible_field_count(&self) -> usize {
        self.groups.iter().map(|g| g.visible_field_count()).sum()
    }

    /// Resolve flat cursor position to (group_index, field_index).
    pub fn cursor_to_group_field(&self) -> Option<(usize, usize)> {
        let mut remaining = self.cursor;
        for (gi, group) in self.groups.iter().enumerate() {
            if group.collapsed {
                continue;
            }
            let count = group.fields.len();
            if remaining < count {
                return Some((gi, remaining));
            }
            remaining -= count;
        }
        None
    }

    /// Find the group index for the current cursor position.
    fn current_group_index(&self) -> Option<usize> {
        self.cursor_to_group_field().map(|(gi, _)| gi)
    }

    /// Get the item count for the current collection field.
    fn current_collection_len(&self) -> usize {
        let Some((gi, fi)) = self.cursor_to_group_field() else { return 0 };
        match &self.groups[gi].fields[fi].value {
            ConfigValue::StringSet(v) => v.len(),
            ConfigValue::StringListMap(v) => v.len(),
            _ => 0,
        }
    }

    /// Handle a semantic input action, producing an action for the dispatch layer.
    pub fn handle_input(&mut self, action: &InputAction) -> ConfigEditorAction {
        // Text input mode intercepts most keys
        if self.text_input.is_some() {
            return self.handle_text_input(action);
        }

        // Sub-collection editing mode (separator list within a StringListMap item)
        if self.sub_collection_pos.is_some() {
            return self.handle_sub_collection_input(action);
        }

        // Collection editing mode
        if self.collection_pos.is_some() {
            return self.handle_collection_input(action);
        }

        match self.focus {
            EditorFocus::Fields => self.handle_fields_input(action),
            EditorFocus::Buttons => self.handle_buttons_input(action),
        }
    }

    /// Handle input while editing within a collection field.
    fn handle_collection_input(&mut self, action: &InputAction) -> ConfigEditorAction {
        let item_count = self.current_collection_len();

        match action {
            InputAction::NavUp => {
                match self.collection_pos {
                    Some(CollectionPosition::Item(0)) => {
                        // Exit collection mode, stay on field
                        self.collection_pos = None;
                    }
                    Some(CollectionPosition::Item(n)) => {
                        self.collection_pos = Some(CollectionPosition::Item(n - 1));
                    }
                    Some(CollectionPosition::AddNew) => {
                        if item_count > 0 {
                            self.collection_pos = Some(CollectionPosition::Item(item_count - 1));
                        } else {
                            self.collection_pos = None;
                        }
                    }
                    None => {}
                }
                ConfigEditorAction::None
            }
            InputAction::NavDown => {
                match self.collection_pos {
                    Some(CollectionPosition::Item(n)) => {
                        if n + 1 < item_count {
                            self.collection_pos = Some(CollectionPosition::Item(n + 1));
                        } else {
                            self.collection_pos = Some(CollectionPosition::AddNew);
                        }
                    }
                    Some(CollectionPosition::AddNew) => {
                        // Exit collection, move to next field
                        self.collection_pos = None;
                        let total = self.visible_field_count();
                        if self.cursor + 1 < total {
                            self.cursor += 1;
                        }
                    }
                    None => {}
                }
                ConfigEditorAction::None
            }
            InputAction::NavLeft | InputAction::NavRight => {
                ConfigEditorAction::None
            }
            InputAction::Confirm => {
                self.activate_collection_item();
                ConfigEditorAction::None
            }
            InputAction::Char('x') | InputAction::Delete => {
                self.delete_collection_item();
                ConfigEditorAction::None
            }
            InputAction::Cancel => {
                self.collection_pos = None;
                ConfigEditorAction::None
            }
            _ => ConfigEditorAction::None,
        }
    }

    /// Get the separator count for the currently focused StringListMap item.
    fn current_sub_collection_len(&self) -> usize {
        let Some((gi, fi)) = self.cursor_to_group_field() else { return 0 };
        let ConfigValue::StringListMap(items) = &self.groups[gi].fields[fi].value else { return 0 };
        let Some(CollectionPosition::Item(idx)) = self.collection_pos else { return 0 };
        if idx < items.len() { items[idx].1.len() } else { 0 }
    }

    /// Handle input while editing within a separator sub-list.
    fn handle_sub_collection_input(&mut self, action: &InputAction) -> ConfigEditorAction {
        let item_count = self.current_sub_collection_len();

        match action {
            InputAction::NavUp => {
                match self.sub_collection_pos {
                    Some(CollectionPosition::Item(0)) => {
                        // Exit sub-collection, stay on the tag item
                        self.sub_collection_pos = None;
                    }
                    Some(CollectionPosition::Item(n)) => {
                        self.sub_collection_pos = Some(CollectionPosition::Item(n - 1));
                    }
                    Some(CollectionPosition::AddNew) => {
                        if item_count > 0 {
                            self.sub_collection_pos = Some(CollectionPosition::Item(item_count - 1));
                        } else {
                            self.sub_collection_pos = None;
                        }
                    }
                    None => {}
                }
                ConfigEditorAction::None
            }
            InputAction::NavDown => {
                match self.sub_collection_pos {
                    Some(CollectionPosition::Item(n)) => {
                        if n + 1 < item_count {
                            self.sub_collection_pos = Some(CollectionPosition::Item(n + 1));
                        } else {
                            self.sub_collection_pos = Some(CollectionPosition::AddNew);
                        }
                    }
                    Some(CollectionPosition::AddNew) => {
                        // Exit sub-collection, move to next tag in collection
                        self.sub_collection_pos = None;
                        let collection_len = self.current_collection_len();
                        match self.collection_pos {
                            Some(CollectionPosition::Item(n)) => {
                                if n + 1 < collection_len {
                                    self.collection_pos = Some(CollectionPosition::Item(n + 1));
                                } else {
                                    self.collection_pos = Some(CollectionPosition::AddNew);
                                }
                            }
                            _ => {}
                        }
                    }
                    None => {}
                }
                ConfigEditorAction::None
            }
            InputAction::Confirm => {
                self.activate_sub_collection_item();
                ConfigEditorAction::None
            }
            InputAction::Char('x') | InputAction::Delete => {
                self.delete_sub_collection_item();
                ConfigEditorAction::None
            }
            InputAction::Cancel => {
                self.sub_collection_pos = None;
                ConfigEditorAction::None
            }
            _ => ConfigEditorAction::None,
        }
    }

    /// Activate (edit) a separator within the sub-collection.
    fn activate_sub_collection_item(&mut self) {
        let Some((gi, fi)) = self.cursor_to_group_field() else { return };
        let ConfigValue::StringListMap(items) = &self.groups[gi].fields[fi].value else { return };
        let Some(CollectionPosition::Item(tag_idx)) = self.collection_pos else { return };
        if tag_idx >= items.len() { return; }

        match self.sub_collection_pos {
            Some(CollectionPosition::Item(sep_idx)) => {
                if sep_idx < items[tag_idx].1.len() {
                    let mut input = TextInputState::new();
                    input.set_value(&items[tag_idx].1[sep_idx]);
                    input.focused = true;
                    self.text_input = Some(input);
                }
            }
            Some(CollectionPosition::AddNew) => {
                let mut input = TextInputState::new();
                input.focused = true;
                self.text_input = Some(input);
            }
            None => {}
        }
    }

    /// Delete a separator within the sub-collection.
    fn delete_sub_collection_item(&mut self) {
        let Some((gi, fi)) = self.cursor_to_group_field() else { return };
        let Some(CollectionPosition::Item(tag_idx)) = self.collection_pos else { return };

        let field = &mut self.groups[gi].fields[fi];
        let ConfigValue::StringListMap(ref mut items) = field.value else { return };
        if tag_idx >= items.len() { return; }

        if let Some(CollectionPosition::Item(sep_idx)) = self.sub_collection_pos {
            if sep_idx < items[tag_idx].1.len() {
                items[tag_idx].1.remove(sep_idx);
                if sep_idx >= items[tag_idx].1.len() && !items[tag_idx].1.is_empty() {
                    self.sub_collection_pos = Some(CollectionPosition::Item(items[tag_idx].1.len() - 1));
                } else if items[tag_idx].1.is_empty() {
                    self.sub_collection_pos = Some(CollectionPosition::AddNew);
                }
                Self::recompute_source(field);
            }
        }
    }

    /// Activate (edit) the current collection item.
    fn activate_collection_item(&mut self) {
        let Some((gi, fi)) = self.cursor_to_group_field() else { return };
        let field = &self.groups[gi].fields[fi];

        match (&field.value, self.collection_pos) {
            (ConfigValue::StringSet(items), Some(CollectionPosition::Item(idx))) => {
                if idx < items.len() {
                    let mut input = TextInputState::new();
                    input.set_value(&items[idx]);
                    input.focused = true;
                    self.text_input = Some(input);
                }
            }
            (ConfigValue::StringSet(_), Some(CollectionPosition::AddNew)) => {
                let mut input = TextInputState::new();
                input.focused = true;
                self.text_input = Some(input);
            }
            (ConfigValue::StringListMap(items), Some(CollectionPosition::Item(idx))) => {
                if idx < items.len() {
                    // Enter sub-collection mode to edit individual separators
                    if items[idx].1.is_empty() {
                        self.sub_collection_pos = Some(CollectionPosition::AddNew);
                    } else {
                        self.sub_collection_pos = Some(CollectionPosition::Item(0));
                    }
                }
            }
            (ConfigValue::StringListMap(_), Some(CollectionPosition::AddNew)) => {
                // Add new tag - first enter tag name
                let mut input = TextInputState::new();
                input.focused = true;
                self.text_input = Some(input);
            }
            _ => {}
        }
    }

    /// Delete the current collection item.
    fn delete_collection_item(&mut self) {
        let Some((gi, fi)) = self.cursor_to_group_field() else { return };
        let field = &mut self.groups[gi].fields[fi];

        let deleted = match (&mut field.value, self.collection_pos) {
            (ConfigValue::StringSet(ref mut items), Some(CollectionPosition::Item(idx))) => {
                if idx < items.len() {
                    items.remove(idx);
                    // Adjust cursor
                    if idx >= items.len() && !items.is_empty() {
                        self.collection_pos = Some(CollectionPosition::Item(items.len() - 1));
                    } else if items.is_empty() {
                        self.collection_pos = Some(CollectionPosition::AddNew);
                    }
                    true
                } else {
                    false
                }
            }
            (ConfigValue::StringListMap(ref mut items), Some(CollectionPosition::Item(idx))) => {
                if idx < items.len() {
                    items.remove(idx);
                    if idx >= items.len() && !items.is_empty() {
                        self.collection_pos = Some(CollectionPosition::Item(items.len() - 1));
                    } else if items.is_empty() {
                        self.collection_pos = Some(CollectionPosition::AddNew);
                    }
                    true
                } else {
                    false
                }
            }
            _ => false,
        };

        if deleted {
            Self::recompute_source(field);
        }
    }

    /// Input handling while navigating fields.
    fn handle_fields_input(&mut self, action: &InputAction) -> ConfigEditorAction {
        let total = self.visible_field_count();
        if total == 0 {
            return ConfigEditorAction::None;
        }

        match action {
            InputAction::NavUp => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
                ConfigEditorAction::None
            }
            InputAction::FocusDown => {
                self.focus = EditorFocus::Buttons;
                ConfigEditorAction::None
            }
            InputAction::NavDown => {
                if self.cursor + 1 < total {
                    self.cursor += 1;
                }
                ConfigEditorAction::None
            }
            InputAction::CyclePrev => {
                self.try_cycle(CycleDirection::Prev)
            }
            InputAction::CycleNext => {
                self.try_cycle(CycleDirection::Next)
            }
            InputAction::Confirm | InputAction::Toggle => {
                self.activate_field();
                ConfigEditorAction::None
            }
            InputAction::NavLeft => {
                self.cycle_enum_left();
                ConfigEditorAction::None
            }
            InputAction::NavRight => {
                self.cycle_enum_right();
                ConfigEditorAction::None
            }
            InputAction::Char('[') => {
                self.jump_to_prev_group();
                ConfigEditorAction::None
            }
            InputAction::Char(']') => {
                self.jump_to_next_group();
                ConfigEditorAction::None
            }
            InputAction::Char('r') => {
                self.reset_current_field();
                ConfigEditorAction::None
            }
            InputAction::Char('C') => {
                self.toggle_current_group_collapse();
                ConfigEditorAction::None
            }
            InputAction::Cancel => {
                ConfigEditorAction::Discard
            }
            _ => ConfigEditorAction::None,
        }
    }

    /// Input handling while focused on buttons.
    fn handle_buttons_input(&mut self, action: &InputAction) -> ConfigEditorAction {
        match action {
            InputAction::NavLeft | InputAction::NavRight | InputAction::CycleNext => {
                self.selected_button = match self.selected_button {
                    EditorButton::Save => EditorButton::Discard,
                    EditorButton::Discard => EditorButton::Save,
                };
                ConfigEditorAction::None
            }
            InputAction::Confirm | InputAction::Toggle => {
                match self.selected_button {
                    EditorButton::Save => ConfigEditorAction::Save,
                    EditorButton::Discard => {
                        // If we got here via Tab with unsaved edits, navigate
                        // to the requested view instead of returning to Insights.
                        match self.pending_cycle.take() {
                            Some(CycleDirection::Next) => ConfigEditorAction::CycleNext,
                            Some(CycleDirection::Prev) => ConfigEditorAction::CyclePrev,
                            None => ConfigEditorAction::Discard,
                        }
                    }
                }
            }
            InputAction::Cancel | InputAction::NavUp | InputAction::NavDown => {
                self.focus = EditorFocus::Fields;
                self.pending_cycle = None;
                ConfigEditorAction::None
            }
            _ => ConfigEditorAction::None,
        }
    }

    /// Attempt a lateral ring cycle. If no edits, cycle immediately.
    /// If edits exist, focus Save/Discard buttons and defer the navigation.
    fn try_cycle(&mut self, direction: CycleDirection) -> ConfigEditorAction {
        if self.has_edits() {
            self.focus = EditorFocus::Buttons;
            self.selected_button = EditorButton::Save;
            self.pending_cycle = Some(direction);
            ConfigEditorAction::None
        } else {
            match direction {
                CycleDirection::Next => ConfigEditorAction::CycleNext,
                CycleDirection::Prev => ConfigEditorAction::CyclePrev,
            }
        }
    }

    /// Input handling while text input is active.
    fn handle_text_input(&mut self, action: &InputAction) -> ConfigEditorAction {
        match action {
            InputAction::Confirm => {
                self.commit_text_input();
                ConfigEditorAction::None
            }
            InputAction::Cancel => {
                self.text_input = None;
                ConfigEditorAction::None
            }
            _ => {
                if let Some(ref mut input) = self.text_input {
                    input.handle_input(action);
                }
                ConfigEditorAction::None
            }
        }
    }

    /// Recompute field source after an edit: if the value matches the original,
    /// restore the original source; otherwise mark as Edited.
    fn recompute_source(field: &mut ConfigField) {
        field.source = if field.value.eq_value(&field.original_value) {
            field.original_source
        } else {
            FieldSource::Edited
        };
    }

    /// Activate the current field (toggle bool, enter text edit, cycle enum, enter collection).
    fn activate_field(&mut self) {
        let Some((gi, fi)) = self.cursor_to_group_field() else { return };

        let field = &mut self.groups[gi].fields[fi];
        match &field.value {
            ConfigValue::Bool(_) => {
                if let ConfigValue::Bool(ref mut b) = field.value {
                    *b = !*b;
                    Self::recompute_source(field);
                }
            }
            ConfigValue::Enum { selected, options } => {
                let len = options.len();
                let new_selected = (*selected + 1) % len;
                if let ConfigValue::Enum { selected: ref mut s, .. } = field.value {
                    *s = new_selected;
                    Self::recompute_source(field);
                }
            }
            ConfigValue::Float(_) | ConfigValue::UintU32(_)
            | ConfigValue::SignedInt(_) | ConfigValue::OptionalUint(_) | ConfigValue::String(_)
            | ConfigValue::Duration(_) => {
                let mut input = TextInputState::new();
                input.set_value(field.value.display());
                input.focused = true;
                self.text_input = Some(input);
            }
            ConfigValue::StringList(_) => {
                let mut input = TextInputState::new();
                input.set_value(field.value.display());
                input.focused = true;
                self.text_input = Some(input);
            }
            // Collection types: enter inline editing mode
            ConfigValue::StringSet(items) => {
                if items.is_empty() {
                    self.collection_pos = Some(CollectionPosition::AddNew);
                } else {
                    self.collection_pos = Some(CollectionPosition::Item(0));
                }
            }
            ConfigValue::StringListMap(items) => {
                if items.is_empty() {
                    self.collection_pos = Some(CollectionPosition::AddNew);
                } else {
                    self.collection_pos = Some(CollectionPosition::Item(0));
                }
            }
        }
    }

    /// Commit text input value to the current field.
    fn commit_text_input(&mut self) {
        let Some(input) = self.text_input.take() else { return };
        let Some((gi, fi)) = self.cursor_to_group_field() else { return };

        let text = input.value;

        // Handle sub-collection (separator) commits
        if let Some(sub_pos) = self.sub_collection_pos {
            let ok = self.commit_sub_collection_input(&text, gi, fi, sub_pos);
            if ok {
                Self::recompute_source(&mut self.groups[gi].fields[fi]);
            }
            return;
        }

        // Handle collection item commits
        if let Some(pos) = self.collection_pos {
            let ok = self.commit_collection_input(&text, gi, fi, pos);
            if ok {
                Self::recompute_source(&mut self.groups[gi].fields[fi]);
            }
            return;
        }

        let field = &mut self.groups[gi].fields[fi];
        let ok = match &mut field.value {
            ConfigValue::Float(ref mut v) => {
                if let Ok(parsed) = text.parse::<f64>() {
                    *v = parsed;
                    true
                } else {
                    false
                }
            }
            ConfigValue::UintU32(ref mut v) => {
                if let Ok(parsed) = text.parse::<u32>() {
                    *v = parsed;
                    true
                } else {
                    false
                }
            }
            ConfigValue::SignedInt(ref mut v) => {
                if let Ok(parsed) = text.parse::<i64>() {
                    *v = parsed;
                    true
                } else {
                    false
                }
            }
            ConfigValue::OptionalUint(ref mut v) => {
                if text.trim().eq_ignore_ascii_case("auto") || text.trim().is_empty() {
                    *v = None;
                    true
                } else if let Ok(parsed) = text.parse::<usize>() {
                    *v = Some(parsed);
                    true
                } else {
                    false
                }
            }
            ConfigValue::Duration(ref mut secs) => {
                let trimmed = text.trim();
                if trimmed.eq_ignore_ascii_case("disabled") || trimmed == "0" {
                    *secs = 0;
                    true
                } else if let Ok(dur) = humantime::parse_duration(trimmed) {
                    *secs = dur.as_secs();
                    true
                } else {
                    false
                }
            }
            ConfigValue::String(ref mut v) => {
                *v = text;
                true
            }
            ConfigValue::StringList(ref mut v) => {
                *v = text.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                true
            }
            // Collection types handled above
            ConfigValue::Bool(_) | ConfigValue::Enum { .. } |
            ConfigValue::StringSet(_) | ConfigValue::StringListMap(_) => false,
        };

        if ok {
            Self::recompute_source(field);
        }
    }

    /// Commit text input for a collection item.
    fn commit_collection_input(&mut self, text: &str, gi: usize, fi: usize, pos: CollectionPosition) -> bool {
        let field = &mut self.groups[gi].fields[fi];
        let text = text.trim();

        match (&mut field.value, pos) {
            (ConfigValue::StringSet(ref mut items), CollectionPosition::Item(idx)) => {
                if idx < items.len() && !text.is_empty() {
                    items[idx] = text.to_string();
                    true
                } else {
                    false
                }
            }
            (ConfigValue::StringSet(ref mut items), CollectionPosition::AddNew) => {
                if !text.is_empty() {
                    items.push(text.to_string());
                    // Move cursor to the new item
                    self.collection_pos = Some(CollectionPosition::Item(items.len() - 1));
                    true
                } else {
                    false
                }
            }
            (ConfigValue::StringListMap(ref mut items), CollectionPosition::Item(idx)) => {
                if idx < items.len() {
                    // Parse comma-separated separators
                    let seps: Vec<String> = text.split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                    items[idx].1 = seps;
                    true
                } else {
                    false
                }
            }
            (ConfigValue::StringListMap(ref mut items), CollectionPosition::AddNew) => {
                if !text.is_empty() {
                    // Adding new tag with empty separators
                    let tag = text.to_uppercase();
                    items.push((tag, Vec::new()));
                    self.collection_pos = Some(CollectionPosition::Item(items.len() - 1));
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    /// Commit text input for a sub-collection separator item.
    fn commit_sub_collection_input(&mut self, text: &str, gi: usize, fi: usize, sub_pos: CollectionPosition) -> bool {
        let field = &mut self.groups[gi].fields[fi];
        let ConfigValue::StringListMap(ref mut items) = field.value else { return false };
        let Some(CollectionPosition::Item(tag_idx)) = self.collection_pos else { return false };
        if tag_idx >= items.len() { return false; }

        // Don't trim — separators can be intentional whitespace
        if text.is_empty() { return false; }

        match sub_pos {
            CollectionPosition::Item(sep_idx) => {
                if sep_idx < items[tag_idx].1.len() {
                    items[tag_idx].1[sep_idx] = text.to_string();
                    true
                } else {
                    false
                }
            }
            CollectionPosition::AddNew => {
                items[tag_idx].1.push(text.to_string());
                self.sub_collection_pos = Some(CollectionPosition::Item(items[tag_idx].1.len() - 1));
                true
            }
        }
    }

    /// Cycle enum left.
    fn cycle_enum_left(&mut self) {
        let Some((gi, fi)) = self.cursor_to_group_field() else { return };
        let field = &mut self.groups[gi].fields[fi];
        if let ConfigValue::Enum { ref mut selected, options } = &mut field.value {
            let len = options.len();
            *selected = if *selected == 0 { len - 1 } else { *selected - 1 };
            Self::recompute_source(field);
        }
    }

    /// Cycle enum right.
    fn cycle_enum_right(&mut self) {
        let Some((gi, fi)) = self.cursor_to_group_field() else { return };
        let field = &mut self.groups[gi].fields[fi];
        if let ConfigValue::Enum { ref mut selected, options } = &mut field.value {
            let len = options.len();
            *selected = (*selected + 1) % len;
            Self::recompute_source(field);
        }
    }

    /// Reset the current field to the value it had when the editor was opened.
    fn reset_current_field(&mut self) {
        let Some((gi, fi)) = self.cursor_to_group_field() else { return };
        let field = &mut self.groups[gi].fields[fi];
        field.value = field.original_value.clone();
        field.source = field.original_source;
    }

    /// Toggle collapsed state of the group containing the cursor.
    fn toggle_current_group_collapse(&mut self) {
        if let Some(gi) = self.current_group_index() {
            self.groups[gi].collapsed = !self.groups[gi].collapsed;
            // Clamp cursor to valid range
            let total = self.visible_field_count();
            if total > 0 && self.cursor >= total {
                self.cursor = total - 1;
            }
        }
    }

    /// Jump cursor to the first field of the next group.
    fn jump_to_next_group(&mut self) {
        let Some(current_gi) = self.current_group_index() else { return };

        // Find next non-collapsed group after current
        let mut offset = 0;
        for (i, group) in self.groups.iter().enumerate() {
            if i <= current_gi {
                if !group.collapsed {
                    offset += group.fields.len();
                }
                continue;
            }
            if !group.collapsed && !group.fields.is_empty() {
                self.cursor = offset;
                return;
            }
            if !group.collapsed {
                offset += group.fields.len();
            }
        }
        // Wrap to first group
        self.cursor = 0;
    }

    /// Jump cursor to the first field of the previous group.
    fn jump_to_prev_group(&mut self) {
        let Some(current_gi) = self.current_group_index() else { return };

        // Find previous non-collapsed group
        for i in (0..current_gi).rev() {
            let group = &self.groups[i];
            if !group.collapsed && !group.fields.is_empty() {
                // offset is currently pointing at current_gi's start
                // We need the start of group i
                let mut target = 0;
                for j in 0..i {
                    if !self.groups[j].collapsed {
                        target += self.groups[j].fields.len();
                    }
                }
                self.cursor = target;
                return;
            }
        }

        // Wrap to last group
        let total = self.visible_field_count();
        if total > 0 {
            // Find start of last non-collapsed group
            let mut pos = total;
            for group in self.groups.iter().rev() {
                if !group.collapsed && !group.fields.is_empty() {
                    pos -= group.fields.len();
                    self.cursor = pos;
                    return;
                }
                if !group.collapsed {
                    pos -= group.fields.len();
                }
            }
        }
    }

    /// Build a new config from the current editor state.
    pub fn build_config(&self) -> Config {
        build::apply_groups_to_config(&self.original_config, &self.groups)
    }
}
