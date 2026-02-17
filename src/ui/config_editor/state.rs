//! Config Editor State and Key Handling
//!
//! Manages cursor navigation, field editing, and the Save/Discard flow.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::config::Config;
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

/// Action produced by `handle_key`, consumed by the action handler.
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
    /// For StringPairMap: which field is focused (0=key, 1=value).
    pub pair_field_focus: usize,
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
            pair_field_focus: 0,
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
            ConfigValue::StringPairMap(v) => v.len(),
            ConfigValue::StringListMap(v) => v.len(),
            _ => 0,
        }
    }

    /// Handle a key event, producing an action for the dispatch layer.
    pub fn handle_key(&mut self, key: KeyEvent) -> ConfigEditorAction {
        // Text input mode intercepts most keys
        if self.text_input.is_some() {
            return self.handle_text_input_key(key);
        }

        // Collection editing mode
        if self.collection_pos.is_some() {
            return self.handle_collection_key(key);
        }

        match self.focus {
            EditorFocus::Fields => self.handle_fields_key(key),
            EditorFocus::Buttons => self.handle_buttons_key(key),
        }
    }

    /// Handle key events while editing within a collection field.
    fn handle_collection_key(&mut self, key: KeyEvent) -> ConfigEditorAction {
        let item_count = self.current_collection_len();

        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
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
            KeyCode::Down | KeyCode::Char('j') => {
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
            KeyCode::Left | KeyCode::Right => {
                // For StringPairMap, switch between key and value
                let Some((gi, fi)) = self.cursor_to_group_field() else {
                    return ConfigEditorAction::None;
                };
                if matches!(self.groups[gi].fields[fi].value, ConfigValue::StringPairMap(_)) {
                    self.pair_field_focus = if self.pair_field_focus == 0 { 1 } else { 0 };
                }
                ConfigEditorAction::None
            }
            KeyCode::Enter => {
                self.activate_collection_item();
                ConfigEditorAction::None
            }
            KeyCode::Char('x') | KeyCode::Delete => {
                self.delete_collection_item();
                ConfigEditorAction::None
            }
            KeyCode::Esc => {
                self.collection_pos = None;
                ConfigEditorAction::None
            }
            _ => ConfigEditorAction::None,
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
            (ConfigValue::StringPairMap(items), Some(CollectionPosition::Item(idx))) => {
                if idx < items.len() {
                    let mut input = TextInputState::new();
                    let value = if self.pair_field_focus == 0 {
                        &items[idx].0
                    } else {
                        &items[idx].1
                    };
                    input.set_value(value);
                    input.focused = true;
                    self.text_input = Some(input);
                }
            }
            (ConfigValue::StringPairMap(_), Some(CollectionPosition::AddNew)) => {
                let mut input = TextInputState::new();
                input.focused = true;
                self.pair_field_focus = 0; // Start with key
                self.text_input = Some(input);
            }
            (ConfigValue::StringListMap(items), Some(CollectionPosition::Item(idx))) => {
                if idx < items.len() {
                    // Edit the separators as comma-separated
                    let mut input = TextInputState::new();
                    input.set_value(items[idx].1.join(", "));
                    input.focused = true;
                    self.text_input = Some(input);
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
            (ConfigValue::StringPairMap(ref mut items), Some(CollectionPosition::Item(idx))) => {
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

    /// Key handling while navigating fields.
    fn handle_fields_key(&mut self, key: KeyEvent) -> ConfigEditorAction {
        let total = self.visible_field_count();
        if total == 0 {
            return ConfigEditorAction::None;
        }

        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
                ConfigEditorAction::None
            }
            KeyCode::Down if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.focus = EditorFocus::Buttons;
                ConfigEditorAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.cursor + 1 < total {
                    self.cursor += 1;
                }
                ConfigEditorAction::None
            }
            KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.try_cycle(CycleDirection::Prev)
            }
            KeyCode::BackTab => {
                self.try_cycle(CycleDirection::Prev)
            }
            KeyCode::Tab => {
                self.try_cycle(CycleDirection::Next)
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                self.activate_field();
                ConfigEditorAction::None
            }
            KeyCode::Left => {
                self.cycle_enum_left();
                ConfigEditorAction::None
            }
            KeyCode::Right => {
                self.cycle_enum_right();
                ConfigEditorAction::None
            }
            KeyCode::Char('[') => {
                self.jump_to_prev_group();
                ConfigEditorAction::None
            }
            KeyCode::Char(']') => {
                self.jump_to_next_group();
                ConfigEditorAction::None
            }
            KeyCode::Char('r') => {
                self.reset_current_field();
                ConfigEditorAction::None
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.toggle_current_group_collapse();
                ConfigEditorAction::None
            }
            KeyCode::Esc => {
                ConfigEditorAction::Discard
            }
            _ => ConfigEditorAction::None,
        }
    }

    /// Key handling while focused on buttons.
    fn handle_buttons_key(&mut self, key: KeyEvent) -> ConfigEditorAction {
        match key.code {
            KeyCode::Left | KeyCode::Right | KeyCode::Tab => {
                self.selected_button = match self.selected_button {
                    EditorButton::Save => EditorButton::Discard,
                    EditorButton::Discard => EditorButton::Save,
                };
                ConfigEditorAction::None
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
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
            KeyCode::Esc | KeyCode::Up | KeyCode::Down => {
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

    /// Key handling while text input is active.
    fn handle_text_input_key(&mut self, key: KeyEvent) -> ConfigEditorAction {
        match key.code {
            KeyCode::Enter => {
                self.commit_text_input();
                ConfigEditorAction::None
            }
            KeyCode::Esc => {
                self.text_input = None;
                ConfigEditorAction::None
            }
            _ => {
                if let Some(ref mut input) = self.text_input {
                    input.handle_key(key);
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
            ConfigValue::Float(_) | ConfigValue::Uint(_) | ConfigValue::UintU32(_)
            | ConfigValue::SignedInt(_) | ConfigValue::OptionalUint(_) | ConfigValue::String(_) => {
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
            ConfigValue::StringPairMap(items) => {
                self.pair_field_focus = 0;
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

        let field = &mut self.groups[gi].fields[fi];
        let text = input.value;

        // Handle collection item commits
        if let Some(pos) = self.collection_pos {
            let ok = self.commit_collection_input(&text, gi, fi, pos);
            if ok {
                Self::recompute_source(&mut self.groups[gi].fields[fi]);
            }
            return;
        }

        let ok = match &mut field.value {
            ConfigValue::Float(ref mut v) => {
                if let Ok(parsed) = text.parse::<f64>() {
                    *v = parsed;
                    true
                } else {
                    false
                }
            }
            ConfigValue::Uint(ref mut v) => {
                if let Ok(parsed) = text.parse::<usize>() {
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
            ConfigValue::StringSet(_) | ConfigValue::StringPairMap(_) | ConfigValue::StringListMap(_) => false,
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
            (ConfigValue::StringPairMap(ref mut items), CollectionPosition::Item(idx)) => {
                if idx < items.len() {
                    if self.pair_field_focus == 0 {
                        if !text.is_empty() {
                            items[idx].0 = text.to_string();
                        }
                    } else {
                        items[idx].1 = text.to_string();
                    }
                    true
                } else {
                    false
                }
            }
            (ConfigValue::StringPairMap(ref mut items), CollectionPosition::AddNew) => {
                if !text.is_empty() {
                    // Adding new pair - this is the key
                    items.push((text.to_string(), String::new()));
                    // Move to the new item's value field
                    self.collection_pos = Some(CollectionPosition::Item(items.len() - 1));
                    self.pair_field_focus = 1;
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
