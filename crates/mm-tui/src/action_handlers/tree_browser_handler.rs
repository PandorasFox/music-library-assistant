//! Tree browser (corpus browser) action handler.

use super::super::App;
use super::witness;
use super::HandleAction;
use mm_meta::decisions::DecisionKey;
use crate::active_view::ActiveView;
use crate::{tree_browser, widgets};

impl HandleAction for tree_browser::TreeBrowserAction {
    fn handle(self, app: &mut App, witness: Option<&witness::ConfirmationGesture>) {
        match self {
            tree_browser::TreeBrowserAction::Cancel => {
                app.start_health_view();
            }
            tree_browser::TreeBrowserAction::EditDirectory(path) => {
                app.push_current_view();
                app.open_unified_tag_editor_for_directory(&path);
            }
            tree_browser::TreeBrowserAction::EditFile(path) => {
                app.push_current_view();
                app.start_tag_editor_for_path(&path, false);
            }
            tree_browser::TreeBrowserAction::CycleNext => {
                app.handle_lateral_cycle(widgets::LateralView::Files, true);
            }
            tree_browser::TreeBrowserAction::CyclePrev => {
                app.handle_lateral_cycle(widgets::LateralView::Files, false);
            }
            tree_browser::TreeBrowserAction::OpenDirConfig(path) => {
                app.open_dir_config_panel(path);
            }
            tree_browser::TreeBrowserAction::SaveDirConfig => {
                if let Some(gesture) = witness {
                    app.save_dir_config(gesture);
                }
            }
            tree_browser::TreeBrowserAction::CloseDirConfig => {
                app.close_dir_config_panel();
            }
            tree_browser::TreeBrowserAction::ReviewTransaction => {
                app.after_staging_decisions();
            }
        }
    }
}

impl App {
    /// Open a dir config panel for the given absolute corpus directory path.
    ///
    /// If an exact SourceDir match exists, loads its values. Otherwise opens
    /// panel with defaults so the user can create a new config entry.
    fn open_dir_config_panel(&mut self, abs_path: std::path::PathBuf) {
        let config = self.config();
        let corpus_dir = config.corpus_dir();

        // Strip corpus_dir prefix to get relative path
        let relative = match abs_path.strip_prefix(&corpus_dir) {
            Ok(r) => r.to_path_buf(),
            Err(_) => return,
        };

        // Find exact matching SourceDir (raw, with Option fields) for editing.
        // We show what THIS dir explicitly sets, not the resolved/inherited values.
        let (libraries, can_stash_dupes, interior_dupes, path_schema, enable_acoustid, pinned_release) =
            match config.get_raw_source_dir(&relative) {
                Some(sd) => (
                    sd.libraries.clone(),
                    sd.can_stash_dupes,
                    sd.interior_dupes,
                    sd.path_schema.as_ref().map(|s| s.template.clone()),
                    sd.enable_acoustid,
                    sd.pinned_release.clone(),
                ),
                None => {
                    // Defaults for a new (unconfigured) directory
                    (vec![], None, None, None, None, None)
                }
            };
        let panel = tree_browser::variants::corpus::DirConfigPanelState {
            source_path: relative,
            libraries: libraries.clone(),
            can_stash_dupes,
            interior_dupes,
            path_schema: path_schema.clone(),
            enable_acoustid,
            pinned_release: pinned_release.clone(),
            orig_libraries: libraries,
            orig_can_stash_dupes: can_stash_dupes,
            orig_interior_dupes: interior_dupes,
            orig_path_schema: path_schema,
            orig_enable_acoustid: enable_acoustid,
            orig_pinned_release: pinned_release,
            field_cursor: 0,
            focus: tree_browser::variants::corpus::PanelFocus::default(),
            button_cursor: 0,
            lib_cursor: None,
            text_input: None,
            wizard_state: crate::widgets::wizard::WizardState::default(),
        };

        if let ActiveView::CorpusBrowser(ref mut browser) = self.view {
            browser.set_config_panel(panel);
        }
    }

    /// Save dir config edits and stage decision.
    fn save_dir_config(&mut self, gesture: &witness::ConfirmationGesture) {
        // Extract panel data from the browser view
        let (source_path, old_dir, new_dir) = {
            let panel = match self.view {
                ActiveView::CorpusBrowser(ref browser) => {
                    match browser.config_panel() {
                        Some(p) => p,
                        None => return,
                    }
                }
                _ => return,
            };

            if !panel.has_edits() {
                // No edits, just close
                self.close_dir_config_panel();
                return;
            }

            let old_dir = mm_meta::config::SourceDir {
                path: panel.source_path.clone(),
                libraries: panel.orig_libraries.clone(),
                can_stash_dupes: panel.orig_can_stash_dupes,
                interior_dupes: panel.orig_interior_dupes,
                path_schema: panel
                    .orig_path_schema
                    .as_ref()
                    .and_then(|t| mm_meta::config::path_schema::parse_path_schema(t).ok()),
                enable_acoustid: panel.orig_enable_acoustid,
                pinned_release: panel.orig_pinned_release.clone(),
            };
            let new_dir = mm_meta::config::SourceDir {
                path: panel.source_path.clone(),
                libraries: panel.libraries.clone(),
                can_stash_dupes: panel.can_stash_dupes,
                interior_dupes: panel.interior_dupes,
                path_schema: panel
                    .path_schema
                    .as_ref()
                    .and_then(|t| mm_meta::config::path_schema::parse_path_schema(t).ok()),
                enable_acoustid: panel.enable_acoustid,
                pinned_release: panel.pinned_release.clone(),
            };
            (panel.source_path.clone(), old_dir, new_dir)
        };

        // Construct the full new Config with the dir edit applied,
        // so the Witch can update SharedConfig in-memory after execution.
        // Default entries (all-defaults, no meaningful config) are elided —
        // they carry no information and will be dropped from dirs.kdl on write.
        let new_config = {
            let mut cfg = (*self.config()).clone();
            let mut found = false;
            for sd in &mut cfg.source_dirs {
                if sd.path == source_path {
                    *sd = new_dir.clone();
                    found = true;
                    break;
                }
            }
            if !found {
                cfg.source_dirs.push(new_dir.clone());
            }
            cfg.source_dirs.retain(|sd| !sd.is_default());
            cfg
        };

        let mutation = mm_meta::mutations::Mutation::ApplyDirConfigEdit(Box::new(
            mm_meta::mutations::dir_config_edit::ApplyDirConfigEditMutation {
                source_path: source_path.clone(),
                old_dir,
                new_dir,
                new_config,
            },
        ));

        let key = DecisionKey::DirConfigEdit {
            source_path: source_path.clone(),
        };
        let label = format!("Dir config: {}", source_path.display());

        let open_txn = self.open_txn_mode();
        if !open_txn {
            let _ = self.witch.start_transaction(&label);
        }
        let decision = gesture.decide(&label, vec![mutation]);
        let _ = super::super::operator_decisions::stage_decision(
            &mut self.witch,
            key,
            decision,
        );

        // Close panel and stay in browser for batch editing
        self.close_dir_config_panel();
        self.sync_browser_pending_edits();
        self.status_message = Some(format!("Dir config staged: {}", source_path.display()));
    }

    /// Close the dir config panel.
    fn close_dir_config_panel(&mut self) {
        if let ActiveView::CorpusBrowser(ref mut browser) = self.view {
            browser.clear_config_panel();
        }
    }

    /// Sync browser's pending-edit markers from the current transaction's DirConfigEdit decisions.
    pub(super) fn sync_browser_pending_edits(&mut self) {
        let mut pending = std::collections::HashSet::new();
        if let Some(ref txn) = self.cached_status.transaction {
            for key in &txn.decision_keys {
                if let DecisionKey::DirConfigEdit { source_path } = key {
                    pending.insert(source_path.clone());
                }
            }
        }
        if let ActiveView::CorpusBrowser(ref mut browser) = self.view {
            browser.set_pending_edit_paths(pending);
        }
    }
}
