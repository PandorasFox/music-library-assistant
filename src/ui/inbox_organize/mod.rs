//! Inbox Organize Module
//!
//! Guided workflow for moving healthy inbox files into the corpus.
//! Walks through inbox directories containing organizable files,
//! presents a corpus directory browser for destination selection,
//! and generates InboxToCorpus mutations.
//!
//! ## Controls (Corpus Browsing Phase)
//!
//! - Up/Down: Navigate corpus tree
//! - Left/Right: Collapse/expand directories
//! - Enter: Select directory (opens emplace popup), or create new directory
//! - S: Skip current directory
//! - Escape: Cancel entire workflow
//!
//! ## Controls (Emplace Popup)
//!
//! - Left/Right: Cycle options (Emplace Dir / Emplace Files / Skip / Cancel)
//! - Enter: Confirm selection
//! - Escape: Return to browsing

pub mod render;

use std::path::PathBuf;

use crate::ui::input::InputAction;

use crate::config::{Config, InboxOrganizeGranularity};
use crate::meta::mutations::{
    file_ops::{InboxDirToCorpusMutation, InboxDirTrackedFile, InboxToCorpusMutation},
    Mutation,
};
use crate::ui::tree_browser::{EntryFilter, TreeNavigator};
use crate::ui::widgets::TextInputState;

pub use mm_meta::views::startup_organize::{InboxDirectory, InboxOrganizeFile};

/// Current phase within the organize workflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrganizePhase {
    /// Browsing corpus tree to pick destination
    BrowsingCorpus,
    /// Text input for new directory name
    NewDirectoryInput,
    /// Confirmation popup: emplace dir / emplace files / skip / cancel
    EmplacePopup,
}

/// Options in the emplace popup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmplaceOption {
    EmplaceDirectory,
    EmplaceFiles,
    Skip,
    Cancel,
}

impl EmplaceOption {
    fn next(self) -> Self {
        match self {
            Self::EmplaceDirectory => Self::EmplaceFiles,
            Self::EmplaceFiles => Self::Skip,
            Self::Skip => Self::Cancel,
            Self::Cancel => Self::EmplaceDirectory,
        }
    }

    fn prev(self) -> Self {
        match self {
            Self::EmplaceDirectory => Self::Cancel,
            Self::EmplaceFiles => Self::EmplaceDirectory,
            Self::Skip => Self::EmplaceFiles,
            Self::Cancel => Self::Skip,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::EmplaceDirectory => "Emplace dir",
            Self::EmplaceFiles => "Emplace files",
            Self::Skip => "Skip",
            Self::Cancel => "Cancel",
        }
    }
}

/// Action returned from input handling.
#[derive(Debug, Clone, PartialEq)]
pub enum InboxOrganizeAction {
    /// No action needed
    None,
    /// Workflow complete — accumulated mutations ready for review
    Complete(Vec<Mutation>),
    /// Workflow cancelled
    Cancel,
}

// ============================================================================
// State
// ============================================================================

/// State for the inbox organize workflow.
#[derive(Debug)]
pub struct InboxOrganizeState {
    /// Inbox directories to process (ordered)
    pub directories: Vec<InboxDirectory>,
    /// Index into `directories` for the current directory
    pub current_dir_idx: usize,
    /// Corpus tree navigator for destination selection
    pub corpus_navigator: TreeNavigator,
    /// Current phase of the workflow
    pub phase: OrganizePhase,
    /// Selected option in the emplace popup
    pub popup_selection: EmplaceOption,
    /// Text input state for new directory name
    pub new_dir_input: TextInputState,
    /// The selected corpus destination path (set when transitioning to popup)
    pub selected_dest: PathBuf,
    /// Mutations accumulated across all directories
    pub accumulated_mutations: Vec<Mutation>,
}

impl InboxOrganizeState {
    /// Create workflow state from pre-loaded directories.
    ///
    /// The directories come from `GetInboxOrganizeData` domain query.
    /// Returns None if directories is empty.
    pub fn from_directories(directories: Vec<InboxDirectory>, config: &Config) -> Option<Self> {
        if directories.is_empty() {
            return None;
        }

        let corpus_dir = config.corpus_dir();

        // Create corpus navigator (directories only, show root, with synthetic entry)
        let mut corpus_navigator = TreeNavigator::new(
            corpus_dir,
            EntryFilter::directories_only(),
            true,
            Vec::new(), // no deploy source paths needed for selection
            Vec::new(), // no zone dimming needed
        );
        corpus_navigator.show_new_dir_entry = true;

        Some(Self {
            directories,
            current_dir_idx: 0,
            corpus_navigator,
            phase: OrganizePhase::BrowsingCorpus,
            popup_selection: EmplaceOption::EmplaceDirectory,
            new_dir_input: TextInputState::default(),
            selected_dest: PathBuf::new(),
            accumulated_mutations: Vec::new(),
        })
    }

    /// Get the current inbox directory being processed.
    pub fn current_dir(&self) -> Option<&InboxDirectory> {
        self.directories.get(self.current_dir_idx)
    }

    /// Progress indicator: "1/N"
    pub fn progress_label(&self) -> String {
        format!("{}/{}", self.current_dir_idx + 1, self.directories.len())
    }

    /// Handle a semantic input action.
    pub fn handle_input(&mut self, action: &InputAction) -> InboxOrganizeAction {
        match self.phase {
            OrganizePhase::BrowsingCorpus => self.handle_browsing_input(action),
            OrganizePhase::NewDirectoryInput => self.handle_new_dir_input(action),
            OrganizePhase::EmplacePopup => self.handle_emplace_input(action),
        }
    }

    // =========================================================================
    // Phase: Browsing Corpus
    // =========================================================================

    fn handle_browsing_input(&mut self, action: &InputAction) -> InboxOrganizeAction {
        match action {
            InputAction::Cancel => InboxOrganizeAction::Cancel,

            InputAction::NavUp => {
                self.corpus_navigator.move_up();
                InboxOrganizeAction::None
            }
            InputAction::NavDown => {
                self.corpus_navigator.move_down();
                InboxOrganizeAction::None
            }
            InputAction::NavRight => {
                self.corpus_navigator.expand_current();
                InboxOrganizeAction::None
            }
            InputAction::NavLeft => {
                self.corpus_navigator.collapse_or_parent();
                InboxOrganizeAction::None
            }

            InputAction::Char('s') | InputAction::Char('S') => self.advance_to_next_dir(),

            InputAction::Confirm => {
                if let Some(entry) = self.corpus_navigator.current_entry().cloned() {
                    if entry.is_synthetic {
                        // "[+ new directory]" — switch to text input
                        self.new_dir_input.clear();
                        // The synthetic entry's path is the parent directory
                        self.selected_dest = entry.path.clone();
                        self.phase = OrganizePhase::NewDirectoryInput;
                    } else if entry.is_directory() {
                        // Select this directory as destination
                        self.selected_dest = entry.path.clone();
                        self.popup_selection = EmplaceOption::EmplaceDirectory;
                        self.phase = OrganizePhase::EmplacePopup;
                    }
                }
                InboxOrganizeAction::None
            }

            _ => InboxOrganizeAction::None,
        }
    }

    // =========================================================================
    // Phase: New Directory Input
    // =========================================================================

    fn handle_new_dir_input(&mut self, action: &InputAction) -> InboxOrganizeAction {
        match action {
            InputAction::Cancel => {
                self.phase = OrganizePhase::BrowsingCorpus;
                InboxOrganizeAction::None
            }
            InputAction::Confirm => {
                let name = self.new_dir_input.value().to_string();
                if name.is_empty() {
                    self.phase = OrganizePhase::BrowsingCorpus;
                } else {
                    // Compute new dir path = selected parent + input name
                    self.selected_dest = self.selected_dest.join(&name);
                    self.popup_selection = EmplaceOption::EmplaceDirectory;
                    self.phase = OrganizePhase::EmplacePopup;
                }
                InboxOrganizeAction::None
            }
            _ => {
                self.new_dir_input.handle_input(action);
                InboxOrganizeAction::None
            }
        }
    }

    // =========================================================================
    // Phase: Emplace Popup
    // =========================================================================

    fn handle_emplace_input(&mut self, action: &InputAction) -> InboxOrganizeAction {
        match action {
            InputAction::Cancel => {
                self.phase = OrganizePhase::BrowsingCorpus;
                InboxOrganizeAction::None
            }
            InputAction::NavLeft => {
                self.popup_selection = self.popup_selection.prev();
                InboxOrganizeAction::None
            }
            InputAction::NavRight => {
                self.popup_selection = self.popup_selection.next();
                InboxOrganizeAction::None
            }
            InputAction::Confirm => match self.popup_selection {
                EmplaceOption::EmplaceDirectory => {
                    self.generate_emplace_directory_mutations();
                    self.register_pending_dest();
                    self.advance_to_next_dir()
                }
                EmplaceOption::EmplaceFiles => {
                    self.generate_emplace_files_mutations();
                    self.register_pending_dest();
                    self.advance_to_next_dir()
                }
                EmplaceOption::Skip => self.advance_to_next_dir(),
                EmplaceOption::Cancel => {
                    self.phase = OrganizePhase::BrowsingCorpus;
                    InboxOrganizeAction::None
                }
            },
            _ => InboxOrganizeAction::None,
        }
    }

    // =========================================================================
    // Pending Directory Tracking
    // =========================================================================

    /// If the selected destination doesn't exist on disk yet, register it as a
    /// pending directory in the tree navigator so it appears for subsequent groups.
    fn register_pending_dest(&mut self) {
        if !self.selected_dest.exists() {
            self.corpus_navigator
                .add_pending_dir(self.selected_dest.clone());
        }
    }

    // =========================================================================
    // Mutation Generation
    // =========================================================================

    /// Emplace directory: move entire `inbox/DirName/` → `corpus/Dest/DirName/`
    ///
    /// Uses a single directory rename so non-audio content (cover images, booklets)
    /// travels with the audio files.
    fn generate_emplace_directory_mutations(&mut self) {
        let Some(current_dir) = self.directories.get(self.current_dir_idx) else {
            return;
        };
        let dest_base = self.selected_dest.join(&current_dir.dir_name);

        let tracked_files: Vec<InboxDirTrackedFile> = current_dir
            .files
            .iter()
            .map(|file| {
                // Compute each file's corpus path by preserving its relative position
                // within the inbox directory (important for TopLevel granularity where
                // files may be in subdirectories).
                let rel = file
                    .path
                    .strip_prefix(&current_dir.dir_path)
                    .unwrap_or(std::path::Path::new(&file.filename));
                InboxDirTrackedFile {
                    inode: file.inode,
                    corpus_path: dest_base.join(rel),
                }
            })
            .collect();

        self.accumulated_mutations
            .push(Mutation::InboxDirToCorpus(InboxDirToCorpusMutation {
                inbox_dir_path: current_dir.dir_path.clone(),
                corpus_dir_path: dest_base,
                tracked_files,
            }));
    }

    /// Emplace files: move individual files → `corpus/Dest/` (flat)
    fn generate_emplace_files_mutations(&mut self) {
        let Some(current_dir) = self.directories.get(self.current_dir_idx) else {
            return;
        };

        for file in &current_dir.files {
            self.accumulated_mutations
                .push(Mutation::InboxToCorpus(InboxToCorpusMutation {
                    inode: file.inode,
                    inbox_path: file.path.clone(),
                    corpus_path: self.selected_dest.join(&file.filename),
                }));
        }
    }

    // =========================================================================
    // Navigation
    // =========================================================================

    /// Advance to the next directory. If all done, return Complete with mutations.
    fn advance_to_next_dir(&mut self) -> InboxOrganizeAction {
        self.current_dir_idx += 1;
        self.phase = OrganizePhase::BrowsingCorpus;

        if self.current_dir_idx >= self.directories.len() {
            // All directories processed
            let mutations = std::mem::take(&mut self.accumulated_mutations);
            if mutations.is_empty() {
                InboxOrganizeAction::Cancel
            } else {
                InboxOrganizeAction::Complete(mutations)
            }
        } else {
            InboxOrganizeAction::None
        }
    }
}

// ============================================================================
// Directory Grouping
// ============================================================================

/// Group organizable files into InboxDirectory structs based on granularity.
pub(crate) fn group_into_directories(
    files: &[(i64, String)],
    inbox_dir: &std::path::Path,
    granularity: InboxOrganizeGranularity,
) -> Vec<InboxDirectory> {
    use crate::corpus::paths;
    use std::collections::BTreeMap;

    let resolver = paths::get_resolver();

    // Convert relative paths to absolute, group by parent directory
    let mut dir_groups: BTreeMap<PathBuf, Vec<InboxOrganizeFile>> = BTreeMap::new();

    for (inode, rel_path) in files {
        let abs_path = resolver.resolve(std::path::Path::new(rel_path));

        let parent = match abs_path.parent() {
            Some(p) => p.to_path_buf(),
            None => continue,
        };
        let filename = abs_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        dir_groups
            .entry(parent)
            .or_default()
            .push(InboxOrganizeFile {
                inode: *inode,
                path: abs_path,
                filename,
            });
    }

    match granularity {
        InboxOrganizeGranularity::Leaf => {
            // Use directories as-is (deepest dirs containing files)
            dir_groups
                .into_iter()
                .map(|(dir_path, files)| {
                    let dir_name = dir_path
                        .strip_prefix(inbox_dir)
                        .unwrap_or(&dir_path)
                        .to_string_lossy()
                        .to_string();
                    InboxDirectory {
                        dir_name,
                        dir_path,
                        files,
                    }
                })
                .collect()
        }
        InboxOrganizeGranularity::TopLevel => {
            // Group by top-level child of inbox/
            let mut top_groups: BTreeMap<PathBuf, Vec<InboxOrganizeFile>> = BTreeMap::new();

            for (dir_path, files) in dir_groups {
                // Find the top-level directory under inbox/
                let rel = dir_path.strip_prefix(inbox_dir).unwrap_or(&dir_path);
                let top_component = rel
                    .components()
                    .next()
                    .map(|c| inbox_dir.join(c.as_os_str()))
                    .unwrap_or_else(|| dir_path.clone());

                top_groups.entry(top_component).or_default().extend(files);
            }

            top_groups
                .into_iter()
                .map(|(dir_path, files)| {
                    let dir_name = dir_path
                        .strip_prefix(inbox_dir)
                        .unwrap_or(&dir_path)
                        .to_string_lossy()
                        .to_string();
                    InboxDirectory {
                        dir_name,
                        dir_path,
                        files,
                    }
                })
                .collect()
        }
    }
}
