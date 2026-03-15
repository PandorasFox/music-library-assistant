//! Config Editor State and Key Handling
//!
//! Manages cursor navigation, field editing, and the Save/Discard flow.

use std::borrow::Cow;

use ratatui::style::Color;

use super::build;
use super::types::*;
use mm_meta::config::Config;
use mm_ui::protocol_binding::ProtocolBinding;
use crate::input::InputAction;
use crate::widgets::modal_buttons::ModalButtons;
use crate::widgets::wizard::{WizardOffer, WizardState};
use crate::widgets::{ButtonRowState, TextInputState};

/// Focus region within the config editor.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EditorFocus {
    Fields,
    Buttons,
}

/// Which button is highlighted when focus is on buttons.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum EditorButton {
    #[default]
    Save,
    Discard,
}

/// Empty context — config editor buttons are always enabled.
#[derive(Debug, Clone, Copy)]
pub struct EditorButtonCtx;

impl ModalButtons for EditorButton {
    type Context = EditorButtonCtx;
    type Action = ConfigEditorAction;

    fn all() -> &'static [Self] {
        &[Self::Save, Self::Discard]
    }

    fn label(&self, _ctx: &Self::Context) -> Cow<'static, str> {
        match self {
            Self::Save => "Save".into(),
            Self::Discard => "Discard".into(),
        }
    }

    fn color(&self, _ctx: &Self::Context) -> Color {
        match self {
            Self::Save => Color::Green,
            Self::Discard => Color::DarkGray,
        }
    }

    fn enabled(&self, _ctx: &Self::Context) -> bool {
        true
    }

    fn action(&self, _ctx: &Self::Context) -> ConfigEditorAction {
        match self {
            Self::Save => ConfigEditorAction::Save,
            Self::Discard => ConfigEditorAction::Discard,
        }
    }

    fn protocol_binding(&self, _ctx: &Self::Context) -> ProtocolBinding {
        match self {
            Self::Save => ProtocolBinding::Transaction {
                decision_key: mm_ui::decision_keys::config_edit(),
                label: "Apply config changes".into(),
            },
            Self::Discard => ProtocolBinding::Navigation,
        }
    }
}

/// Action produced by `handle_input`, consumed by the action handler.
///
/// ConfigEditor keeps CycleNext/CyclePrev as domain actions because Tab
/// behavior depends on unsaved edits (defers cycle → focuses Save/Discard).
pub enum ConfigEditorAction {
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

/// Navigate a collection cursor up. Returns true if the cursor exited the collection.
fn collection_nav_up(pos: &mut Option<CollectionPosition>, item_count: usize) -> bool {
    match *pos {
        Some(CollectionPosition::Item(0)) => {
            *pos = None;
            true
        }
        Some(CollectionPosition::Item(n)) => {
            *pos = Some(CollectionPosition::Item(n - 1));
            false
        }
        Some(CollectionPosition::AddNew) => {
            *pos = if item_count > 0 {
                Some(CollectionPosition::Item(item_count - 1))
            } else {
                None
            };
            pos.is_none()
        }
        None => false,
    }
}

/// Navigate a collection cursor down. Returns true if the cursor exited the collection.
fn collection_nav_down(pos: &mut Option<CollectionPosition>, item_count: usize) -> bool {
    match *pos {
        Some(CollectionPosition::Item(n)) => {
            *pos = if n + 1 < item_count {
                Some(CollectionPosition::Item(n + 1))
            } else {
                Some(CollectionPosition::AddNew)
            };
            false
        }
        Some(CollectionPosition::AddNew) => {
            *pos = None;
            true
        }
        None => false,
    }
}

/// Adjust cursor after deleting an item at the given index.
fn adjust_cursor_after_delete(pos: &mut Option<CollectionPosition>, new_len: usize) {
    if let Some(CollectionPosition::Item(idx)) = *pos {
        if idx >= new_len && new_len > 0 {
            *pos = Some(CollectionPosition::Item(new_len - 1));
        } else if new_len == 0 {
            *pos = Some(CollectionPosition::AddNew);
        }
    }
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
    pub buttons: ButtonRowState<EditorButton>,
    /// When set, Discard navigates laterally instead of returning to Insights.
    /// Set by Tab/Shift-Tab when there are unsaved edits.
    pub pending_cycle: Option<CycleDirection>,
    /// Wizard popup state for Z-key help text.
    pub wizard_state: WizardState,
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
            buttons: ButtonRowState::new(), // defaults to Save
            pending_cycle: None,
            wizard_state: WizardState::default(),
        }
    }

    /// Check if any field has been edited.
    pub fn has_edits(&self) -> bool {
        self.groups
            .iter()
            .any(|g| g.fields.iter().any(|f| f.source == FieldSource::Edited))
    }

    /// Total number of cursor-navigable slots across all groups.
    /// Expanded groups contribute their field count; collapsed groups contribute 1
    /// (for the collapsed header, so the user can navigate to it and expand).
    fn visible_field_count(&self) -> usize {
        self.groups
            .iter()
            .map(|g| if g.collapsed { 1 } else { g.fields.len() })
            .sum()
    }

    /// Resolve flat cursor position to (group_index, field_index).
    /// Returns `None` if the cursor is on a collapsed group header (no field to select).
    pub fn cursor_to_group_field(&self) -> Option<(usize, usize)> {
        let mut remaining = self.cursor;
        for (gi, group) in self.groups.iter().enumerate() {
            if group.collapsed {
                if remaining == 0 {
                    return None; // cursor is on collapsed group header
                }
                remaining -= 1;
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
    /// Works for both expanded groups (cursor on a field) and collapsed groups
    /// (cursor on the collapsed header slot).
    fn current_group_index(&self) -> Option<usize> {
        let mut remaining = self.cursor;
        for (gi, group) in self.groups.iter().enumerate() {
            if group.collapsed {
                if remaining == 0 {
                    return Some(gi);
                }
                remaining -= 1;
                continue;
            }
            let count = group.fields.len();
            if remaining < count {
                return Some(gi);
            }
            remaining -= count;
        }
        None
    }

    /// Get the item count for the current collection field.
    fn current_collection_len(&self) -> usize {
        let Some((gi, fi)) = self.cursor_to_group_field() else {
            return 0;
        };
        match &self.groups[gi].fields[fi].value {
            ConfigValue::StringSet(v) => v.len(),
            ConfigValue::StringListMap(v) => v.len(),
            _ => 0,
        }
    }

    /// Handle a semantic input action, producing an action for the dispatch layer.
    /// ConfigEditor keeps CycleNext/CyclePrev as domain actions (complex Tab behavior).
    pub fn handle_input(&mut self, action: &InputAction) -> Option<ConfigEditorAction> {
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
    fn handle_collection_input(&mut self, action: &InputAction) -> Option<ConfigEditorAction> {
        let item_count = self.current_collection_len();

        match action {
            InputAction::NavUp => {
                collection_nav_up(&mut self.collection_pos, item_count);
                None
            }
            InputAction::NavDown => {
                if collection_nav_down(&mut self.collection_pos, item_count) {
                    // Exited collection, move to next field
                    let total = self.visible_field_count();
                    if self.cursor + 1 < total {
                        self.cursor += 1;
                    }
                }
                None
            }
            InputAction::NavLeft | InputAction::NavRight => None,
            InputAction::Confirm => {
                self.activate_collection_item();
                None
            }
            InputAction::Char('x') | InputAction::Delete => {
                self.delete_collection_item();
                None
            }
            InputAction::Cancel => {
                self.collection_pos = None;
                None
            }
            _ => None,
        }
    }

    /// Get the separator count for the currently focused StringListMap item.
    fn current_sub_collection_len(&self) -> usize {
        let Some((gi, fi)) = self.cursor_to_group_field() else {
            return 0;
        };
        let ConfigValue::StringListMap(items) = &self.groups[gi].fields[fi].value else {
            return 0;
        };
        let Some(CollectionPosition::Item(idx)) = self.collection_pos else {
            return 0;
        };
        if idx < items.len() {
            items[idx].1.len()
        } else {
            0
        }
    }

    /// Handle input while editing within a separator sub-list.
    fn handle_sub_collection_input(&mut self, action: &InputAction) -> Option<ConfigEditorAction> {
        let item_count = self.current_sub_collection_len();

        match action {
            InputAction::NavUp => {
                collection_nav_up(&mut self.sub_collection_pos, item_count);
                None
            }
            InputAction::NavDown => {
                if collection_nav_down(&mut self.sub_collection_pos, item_count) {
                    // Exited sub-collection, advance parent collection cursor
                    let collection_len = self.current_collection_len();
                    collection_nav_down(&mut self.collection_pos, collection_len);
                }
                None
            }
            InputAction::Confirm => {
                self.activate_sub_collection_item();
                None
            }
            InputAction::Char('x') | InputAction::Delete => {
                self.delete_sub_collection_item();
                None
            }
            InputAction::Cancel => {
                self.sub_collection_pos = None;
                None
            }
            _ => None,
        }
    }

    /// Activate (edit) a separator within the sub-collection.
    fn activate_sub_collection_item(&mut self) {
        let Some((gi, fi)) = self.cursor_to_group_field() else {
            return;
        };
        let ConfigValue::StringListMap(items) = &self.groups[gi].fields[fi].value else {
            return;
        };
        let Some(CollectionPosition::Item(tag_idx)) = self.collection_pos else {
            return;
        };
        if tag_idx >= items.len() {
            return;
        }

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
        let Some((gi, fi)) = self.cursor_to_group_field() else {
            return;
        };
        let Some(CollectionPosition::Item(tag_idx)) = self.collection_pos else {
            return;
        };

        let field = &mut self.groups[gi].fields[fi];
        let ConfigValue::StringListMap(ref mut items) = field.value else {
            return;
        };
        if tag_idx >= items.len() {
            return;
        }

        if let Some(CollectionPosition::Item(sep_idx)) = self.sub_collection_pos {
            if sep_idx < items[tag_idx].1.len() {
                items[tag_idx].1.remove(sep_idx);
                adjust_cursor_after_delete(&mut self.sub_collection_pos, items[tag_idx].1.len());
                Self::recompute_source(field);
            }
        }
    }

    /// Activate (edit) the current collection item.
    fn activate_collection_item(&mut self) {
        let Some((gi, fi)) = self.cursor_to_group_field() else {
            return;
        };
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
        let Some((gi, fi)) = self.cursor_to_group_field() else {
            return;
        };
        let field = &mut self.groups[gi].fields[fi];

        let deleted = match (&mut field.value, self.collection_pos) {
            (ConfigValue::StringSet(ref mut items), Some(CollectionPosition::Item(idx))) => {
                if idx < items.len() {
                    items.remove(idx);
                    adjust_cursor_after_delete(&mut self.collection_pos, items.len());
                    true
                } else {
                    false
                }
            }
            (ConfigValue::StringListMap(ref mut items), Some(CollectionPosition::Item(idx))) => {
                if idx < items.len() {
                    items.remove(idx);
                    adjust_cursor_after_delete(&mut self.collection_pos, items.len());
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
    fn handle_fields_input(&mut self, action: &InputAction) -> Option<ConfigEditorAction> {
        let total = self.visible_field_count();
        if total == 0 {
            return None;
        }

        match action {
            InputAction::NavUp => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                    self.wizard_state.dismiss();
                }
                None
            }
            InputAction::FocusDown => {
                self.focus = EditorFocus::Buttons;
                None
            }
            InputAction::NavDown => {
                if self.cursor + 1 < total {
                    self.cursor += 1;
                    self.wizard_state.dismiss();
                }
                None
            }
            InputAction::CyclePrev => self.try_cycle(CycleDirection::Prev),
            InputAction::CycleNext => self.try_cycle(CycleDirection::Next),
            InputAction::Confirm | InputAction::Toggle => {
                self.activate_field();
                None
            }
            InputAction::NavLeft => {
                self.cycle_enum_left();
                None
            }
            InputAction::NavRight => {
                self.cycle_enum_right();
                None
            }
            InputAction::Char('[') => {
                self.jump_to_prev_group();
                self.wizard_state.dismiss();
                None
            }
            InputAction::Char(']') => {
                self.jump_to_next_group();
                self.wizard_state.dismiss();
                None
            }
            InputAction::Char('r') => {
                self.reset_current_field();
                None
            }
            InputAction::Char('C') => {
                self.toggle_current_group_collapse();
                None
            }
            InputAction::Char('z') | InputAction::Char('Z') => {
                if let Some((gi, fi)) = self.cursor_to_group_field() {
                    let help = self.groups[gi].fields[fi].help;
                    if !help.is_empty() {
                        let lines: Vec<ratatui::text::Line<'static>> =
                            help.iter().map(|s| ratatui::text::Line::raw(s.to_string())).collect();
                        let offer = WizardOffer::Popup(lines);
                        self.wizard_state.advance(&offer);
                    }
                }
                None
            }
            InputAction::Cancel => Some(ConfigEditorAction::Discard),
            _ => None,
        }
    }

    /// Input handling while focused on buttons.
    fn handle_buttons_input(&mut self, action: &InputAction) -> Option<ConfigEditorAction> {
        let ctx = EditorButtonCtx;
        match action {
            InputAction::NavLeft => {
                self.buttons.nav_left(&ctx);
                None
            }
            InputAction::NavRight | InputAction::CycleNext => {
                self.buttons.nav_right(&ctx);
                None
            }
            InputAction::Confirm | InputAction::Toggle => {
                match self.buttons.confirm(&ctx) {
                    Some(ConfigEditorAction::Discard) => {
                        // If we got here via Tab with unsaved edits, navigate
                        // to the requested view instead of returning to Insights.
                        match self.pending_cycle.take() {
                            Some(CycleDirection::Next) => Some(ConfigEditorAction::CycleNext),
                            Some(CycleDirection::Prev) => Some(ConfigEditorAction::CyclePrev),
                            None => Some(ConfigEditorAction::Discard),
                        }
                    }
                    Some(action) => Some(action),
                    None => None,
                }
            }
            InputAction::Cancel | InputAction::NavUp | InputAction::NavDown => {
                self.focus = EditorFocus::Fields;
                self.pending_cycle = None;
                None
            }
            _ => None,
        }
    }

    /// Attempt a lateral ring cycle. If no edits, cycle immediately.
    /// If edits exist, focus Save/Discard buttons and defer the navigation.
    fn try_cycle(&mut self, direction: CycleDirection) -> Option<ConfigEditorAction> {
        if self.has_edits() {
            self.focus = EditorFocus::Buttons;
            self.buttons.selected = EditorButton::Save;
            self.pending_cycle = Some(direction);
            None
        } else {
            match direction {
                CycleDirection::Next => Some(ConfigEditorAction::CycleNext),
                CycleDirection::Prev => Some(ConfigEditorAction::CyclePrev),
            }
        }
    }

    /// Input handling while text input is active.
    fn handle_text_input(&mut self, action: &InputAction) -> Option<ConfigEditorAction> {
        match action {
            InputAction::Confirm => {
                self.commit_text_input();
                None
            }
            InputAction::Cancel => {
                self.text_input = None;
                None
            }
            _ => {
                if let Some(ref mut input) = self.text_input {
                    input.handle_input(action);
                }
                None
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
        let Some((gi, fi)) = self.cursor_to_group_field() else {
            return;
        };

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
                if let ConfigValue::Enum {
                    selected: ref mut s,
                    ..
                } = field.value
                {
                    *s = new_selected;
                    Self::recompute_source(field);
                }
            }
            ConfigValue::Float(_)
            | ConfigValue::UintU32(_)
            | ConfigValue::SignedInt(_)
            | ConfigValue::OptionalUint(_)
            | ConfigValue::String(_)
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
        let Some(input) = self.text_input.take() else {
            return;
        };
        let Some((gi, fi)) = self.cursor_to_group_field() else {
            return;
        };

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

        macro_rules! try_parse_into {
            ($text:expr, $target:expr, $type:ty) => {
                if let Ok(parsed) = $text.parse::<$type>() {
                    *$target = parsed;
                    true
                } else {
                    false
                }
            };
        }

        let ok = match &mut field.value {
            ConfigValue::Float(v) => try_parse_into!(text, v, f64),
            ConfigValue::UintU32(v) => try_parse_into!(text, v, u32),
            ConfigValue::SignedInt(v) => try_parse_into!(text, v, i64),
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
                *v = text
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                true
            }
            // Collection types handled above
            ConfigValue::Bool(_)
            | ConfigValue::Enum { .. }
            | ConfigValue::StringSet(_)
            | ConfigValue::StringListMap(_) => false,
        };

        if ok {
            Self::recompute_source(field);
        }
    }

    /// Commit text input for a collection item.
    fn commit_collection_input(
        &mut self,
        text: &str,
        gi: usize,
        fi: usize,
        pos: CollectionPosition,
    ) -> bool {
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
                    let seps: Vec<String> = text
                        .split(',')
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
    fn commit_sub_collection_input(
        &mut self,
        text: &str,
        gi: usize,
        fi: usize,
        sub_pos: CollectionPosition,
    ) -> bool {
        let field = &mut self.groups[gi].fields[fi];
        let ConfigValue::StringListMap(ref mut items) = field.value else {
            return false;
        };
        let Some(CollectionPosition::Item(tag_idx)) = self.collection_pos else {
            return false;
        };
        if tag_idx >= items.len() {
            return false;
        }

        // Don't trim — separators can be intentional whitespace
        if text.is_empty() {
            return false;
        }

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
                self.sub_collection_pos =
                    Some(CollectionPosition::Item(items[tag_idx].1.len() - 1));
                true
            }
        }
    }

    /// Cycle enum left.
    fn cycle_enum_left(&mut self) {
        let Some((gi, fi)) = self.cursor_to_group_field() else {
            return;
        };
        let field = &mut self.groups[gi].fields[fi];
        if let ConfigValue::Enum {
            ref mut selected,
            options,
        } = &mut field.value
        {
            let len = options.len();
            *selected = if *selected == 0 {
                len - 1
            } else {
                *selected - 1
            };
            Self::recompute_source(field);
        }
    }

    /// Cycle enum right.
    fn cycle_enum_right(&mut self) {
        let Some((gi, fi)) = self.cursor_to_group_field() else {
            return;
        };
        let field = &mut self.groups[gi].fields[fi];
        if let ConfigValue::Enum {
            ref mut selected,
            options,
        } = &mut field.value
        {
            let len = options.len();
            *selected = (*selected + 1) % len;
            Self::recompute_source(field);
        }
    }

    /// Reset the current field to the value it had when the editor was opened.
    fn reset_current_field(&mut self) {
        let Some((gi, fi)) = self.cursor_to_group_field() else {
            return;
        };
        let field = &mut self.groups[gi].fields[fi];
        field.value = field.original_value.clone();
        field.source = field.original_source;
    }

    /// Toggle collapsed state of the group containing the cursor.
    fn toggle_current_group_collapse(&mut self) {
        let Some(gi) = self.current_group_index() else {
            return;
        };
        self.groups[gi].collapsed = !self.groups[gi].collapsed;
        // After toggling, reposition cursor to the start of this group.
        // This keeps the cursor on the group whether expanding or collapsing.
        self.cursor = self.group_start_offset(gi);
    }

    /// Compute the flat cursor offset for the start of a group (first field,
    /// or the collapsed-header slot).
    fn group_start_offset(&self, target_gi: usize) -> usize {
        let mut offset = 0;
        for (i, group) in self.groups.iter().enumerate() {
            if i == target_gi {
                return offset;
            }
            offset += if group.collapsed {
                1
            } else {
                group.fields.len()
            };
        }
        offset
    }

    /// Jump cursor to the first slot of the next group.
    fn jump_to_next_group(&mut self) {
        let Some(current_gi) = self.current_group_index() else {
            return;
        };
        // Find next group with content (or collapsed header)
        for i in (current_gi + 1)..self.groups.len() {
            if self.groups[i].collapsed || !self.groups[i].fields.is_empty() {
                self.cursor = self.group_start_offset(i);
                return;
            }
        }
        // Wrap to first group
        self.cursor = 0;
    }

    /// Jump cursor to the first slot of the previous group.
    fn jump_to_prev_group(&mut self) {
        let Some(current_gi) = self.current_group_index() else {
            return;
        };
        // Find previous group with content (or collapsed header)
        for i in (0..current_gi).rev() {
            if self.groups[i].collapsed || !self.groups[i].fields.is_empty() {
                self.cursor = self.group_start_offset(i);
                return;
            }
        }
        // Wrap to last group
        for i in (0..self.groups.len()).rev() {
            if self.groups[i].collapsed || !self.groups[i].fields.is_empty() {
                self.cursor = self.group_start_offset(i);
                return;
            }
        }
    }

    /// Build a new config from the current editor state.
    pub fn build_config(&self) -> Config {
        build::apply_groups_to_config(&self.original_config, &self.groups)
    }
}
