//! Compound Tag Split Resolution
//!
//! Handles the V3 compound tag split modal: single-load packed data with
//! StandardList + DecisionField + ButtonRow.

use super::super::App;
use super::witness;
use super::HandleAction;
use mm_meta::db_types::Zone;
use mm_ui::modal_buttons::ModalButtons;
use mm_ui::resolutions::compound_split::{CompoundSplitButton, CompoundSplitButtonCtx};
use crate::ActiveView;

// =========================================================================
// V3: Single-load compound split with packed data
// =========================================================================

impl HandleAction for mm_ui::resolutions::compound_split::CompoundSplitAction {
    fn handle(self, app: &mut App, witness: Option<&witness::ConfirmationGesture>) {
        use mm_ui::resolutions::compound_split::CompoundSplitAction;
        match self {
            CompoundSplitAction::Confirm => {
                let Some(w) = witness else { return };
                app.stage_compound_split_v3(w);
                app.advance_compound_split_v3();
            }
            CompoundSplitAction::Canonicalize => {
                let Some(w) = witness else { return };
                app.stage_compound_canonicalize_v3(w);
                app.advance_compound_split_v3();
            }
            CompoundSplitAction::Cancel => {
                app.cancel_and_return_to_source("Compound tag split cancelled");
            }
        }
    }
}

impl App {
    /// Start compound split resolution using the packed query (V3).
    pub(crate) fn start_compound_split_resolution_v3(
        &mut self,
        safe_only: bool,
        tag_filter: Option<&str>,
    ) {
        self.start_compound_split_resolution_v3_for_zone(safe_only, tag_filter, Zone::Corpus);
    }

    /// Start compound split resolution V3 for a given zone.
    fn start_compound_split_resolution_v3_for_zone(
        &mut self,
        safe_only: bool,
        tag_filter: Option<&str>,
        zone: Zone,
    ) {
        // Determine tag_name: use filter if provided, otherwise empty string to load all
        let tag_name = tag_filter.unwrap_or("").to_string();

        // Single packed query -- all groups at once
        let data = self.query(mm_meta::domain_queries::GetCompoundSplitResolution {
            tag_name: tag_name.clone(),
            zone,
            safe_only,
        });

        if data.groups.is_empty() {
            self.status_message = Some("No compound tag signals to resolve".to_string());
            return;
        }

        // Start transaction
        let mode_str = if zone == Zone::Inbox {
            "inbox"
        } else if safe_only {
            "safe"
        } else {
            "review"
        };
        let _ = self.start_transaction(&format!("Compound tag split ({})", mode_str));

        // Pre-fill DecisionField with first group's split parts
        let prefill = data.groups.first()
            .map(|g| g.split_parts.join("; "))
            .unwrap_or_default();
        let field = mm_ui::decision_field::DecisionField::new("Split parts:")
            .with_value(&prefill);

        self.view = ActiveView::CompoundTagSplitResolution {
            data,
            current_group: 0,
            list: mm_ui::standard_list::StandardListState::new(
                mm_ui::standard_list::StandardListConfig::default(),
            ),
            buttons: mm_ui::modal_buttons::ButtonRowState::new(),
            field,
            zone,
            focus: mm_ui::geometry::FocusPane::List,
            safe_mode: safe_only,
        };
    }

    /// Advance to next group or go to review (V3).
    fn advance_compound_split_v3(&mut self) {
        if let ActiveView::CompoundTagSplitResolution {
            ref data, ref mut current_group, ref mut list, ref mut field, ..
        } = self.view
        {
            if *current_group + 1 < data.groups.len() {
                *current_group += 1;
                list.reset();
                // Pre-fill field with new group's split parts
                if let Some(group) = data.groups.get(*current_group) {
                    field.set_value(&group.split_parts.join("; "));
                }
            } else {
                // Last group -- go to review
                self.after_staging_decisions();
            }
        } else {
            self.after_staging_decisions();
        }
    }

    /// Build a `CompoundSplitButtonCtx` from the current view state.
    fn compound_split_ctx(&self) -> Option<CompoundSplitButtonCtx> {
        match &self.view {
            ActiveView::CompoundTagSplitResolution {
                ref data, current_group, zone, safe_mode, ..
            } => {
                let tag_name = data.groups
                    .get(*current_group)
                    .map(|g| g.tag_name.clone())
                    .unwrap_or_default();
                Some(CompoundSplitButtonCtx {
                    has_files: true,
                    current_group_index: *current_group,
                    tag_name,
                    zone: *zone,
                    safe_mode: *safe_mode,
                })
            }
            _ => None,
        }
    }

    /// Stage compound split decision for current group (V3).
    ///
    /// Parses the DecisionField value as semicolon-separated split parts,
    /// then builds tag ops: replace compound value with first part, add remaining parts.
    fn stage_compound_split_v3(&mut self, gesture: &witness::ConfirmationGesture) {
        let (mutations, description) = match &self.view {
            ActiveView::CompoundTagSplitResolution {
                ref data, current_group, ref field, ref zone, ..
            } => {
                use mm_meta::mutations::tag_edit::ApplyTagOpsMutation;
                use mm_meta::mutations::{Mutation, TagOp};

                let group = match data.groups.get(*current_group) {
                    Some(g) => g,
                    None => return,
                };

                // Parse field value into parts
                let parts: Vec<String> = field.value()
                    .split(';')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();

                if parts.is_empty() {
                    return;
                }

                // Build tag ops for each file
                let mut ops = Vec::new();
                for file in &group.files {
                    // Replace compound value with first part
                    if let Some(first_part) = parts.first() {
                        ops.push(TagOp::replace_tag(
                            file.inode,
                            &group.tag_name,
                            &group.compound_value,
                            first_part,
                        ));
                    }
                    // Add remaining parts
                    for part in parts.iter().skip(1) {
                        ops.push(TagOp::add_tag(
                            file.inode,
                            &group.tag_name,
                            part,
                        ));
                    }
                }

                if ops.is_empty() {
                    return;
                }

                let mutations = vec![Mutation::ApplyTagOps(ApplyTagOpsMutation {
                    ops,
                    zone: *zone,
                })];

                let desc = format!(
                    "Split \"{}\" in {} \u{2192} [{}]",
                    group.compound_value,
                    group.tag_name,
                    parts.join(", "),
                );
                (mutations, desc)
            }
            _ => return,
        };

        let ctx = match self.compound_split_ctx() {
            Some(c) => c,
            None => return,
        };
        let key = CompoundSplitButton::Confirm.protocol_binding(&ctx)
            .decision_key().unwrap().clone();
        let decision = gesture.decide(&description, mutations);
        let _ = super::super::operator_decisions::stage_decision(self, key, decision);
    }

    /// Stage a canonicalize decision for current group (V3).
    ///
    /// Emits an EmitCanonicalTag mutation to mark the compound value as a
    /// standalone entity, suppressing future compound detection for it.
    fn stage_compound_canonicalize_v3(&mut self, gesture: &witness::ConfirmationGesture) {
        let (mutations, description) = match &self.view {
            ActiveView::CompoundTagSplitResolution {
                ref data, current_group, ..
            } => {
                use mm_meta::mutations::indexing::EmitCanonicalTagMutation;
                use mm_meta::mutations::Mutation;

                let group = match data.groups.get(*current_group) {
                    Some(g) => g,
                    None => return,
                };

                let mutations = vec![Mutation::EmitCanonicalTag(EmitCanonicalTagMutation {
                    tag_name: group.tag_name.clone(),
                    canonical_value: group.compound_value.clone(),
                })];

                let desc = format!(
                    "Keep \"{}\" in {} as canonical",
                    group.compound_value, group.tag_name,
                );
                (mutations, desc)
            }
            _ => return,
        };

        let ctx = match self.compound_split_ctx() {
            Some(c) => c,
            None => return,
        };
        let key = CompoundSplitButton::Canonicalize.protocol_binding(&ctx)
            .decision_key().unwrap().clone();
        let decision = gesture.decide(&description, mutations);
        let _ = super::super::operator_decisions::stage_decision(self, key, decision);
    }
}
