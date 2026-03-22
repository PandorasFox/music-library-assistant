//! OOB Resolution — types and state for out-of-band tag resolution.
//!
//! Unified module replacing the former oob_sync + oob_conflict split.
//! All OOB signal files are classified into `ConflictBucket` categories
//! and resolved through a single tabbed interface.
//!
//! Route: `/resolve/oob-resolution`
//! Query: `GetOobFiles`

use std::borrow::Cow;
use std::collections::BTreeSet;

use ratatui::style::{Color, Style};

use mm_meta::decisions::DecisionKey;
use mm_meta::views::{ConflictBucket, OobFile, TagMismatchEntry};

use crate::geometry::FocusPane;
use crate::input::InputAction;
use crate::modal_buttons::ModalButtons;
use crate::modal_frame::{ContentLayout, FrameInputResult, FrameState, ModalFrameCore};
use crate::protocol_binding::ProtocolBinding;
use crate::rich_text::{RichBlock, RichSpan};
use crate::standard_list::{ListEntry, StandardListConfig, StandardListState};
use crate::wizard::{WizardItem, WizardOffer};

// ============================================================================
// ListEntry + WizardItem for OobFile
// ============================================================================

impl ListEntry for OobFile {
    type Action = OobAction;

    fn on_confirm(&self, _selected: &BTreeSet<usize>) -> Option<OobAction> {
        None // buttons handle resolution, not list items
    }
}

impl WizardItem for OobFile {
    fn wizard(&self, _width: u16) -> Option<WizardOffer> {
        if self.mismatches.is_empty() {
            return None;
        }

        let mut content = Vec::new();
        for m in &self.mismatches {
            let db = m.db_value.as_deref().unwrap_or("\u{2014}");
            let disk = m.disk_value.as_deref().unwrap_or("\u{2014}");
            content.push(RichBlock::Paragraph(vec![
                RichSpan::new(&m.field, Style::default().fg(Color::White)),
                RichSpan::new(": ", Style::default().fg(Color::DarkGray)),
                RichSpan::new(db, Style::default().fg(Color::Green)),
                RichSpan::new(" \u{2192} ", Style::default().fg(Color::DarkGray)),
                RichSpan::new(disk, Style::default().fg(Color::Cyan)),
            ]));
        }

        Some(WizardOffer::Pane {
            title: format!("Tag Diff: {}", self.path),
            content,
        })
    }
}

// ============================================================================
// Button types
// ============================================================================

#[derive(Debug, Clone, Copy)]
pub struct OobButtonCtx {
    pub is_resolvable: bool,
    pub is_acknowledgeable: bool,
    pub has_files: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OobButton {
    #[default]
    ApplyDb,
    AssimilateDisk,
    Acknowledge,
    Cancel,
}

impl ModalButtons for OobButton {
    type Context = OobButtonCtx;
    type Action = OobAction;

    fn all() -> &'static [Self] {
        &[Self::ApplyDb, Self::AssimilateDisk, Self::Acknowledge, Self::Cancel]
    }

    fn label(&self, _ctx: &Self::Context) -> Cow<'static, str> {
        match self {
            Self::ApplyDb => "Apply DB -> Files".into(),
            Self::AssimilateDisk => "Assimilate Files -> DB".into(),
            Self::Acknowledge => "Acknowledge Mtime".into(),
            Self::Cancel => "Cancel".into(),
        }
    }

    fn color(&self, _ctx: &Self::Context) -> Color {
        match self {
            Self::ApplyDb => Color::Green,
            Self::AssimilateDisk => Color::Cyan,
            Self::Acknowledge => Color::Green,
            Self::Cancel => Color::White,
        }
    }

    fn enabled(&self, ctx: &Self::Context) -> bool {
        match self {
            Self::ApplyDb => ctx.is_resolvable && ctx.has_files,
            Self::AssimilateDisk => ctx.is_resolvable && ctx.has_files,
            Self::Acknowledge => ctx.is_acknowledgeable && ctx.has_files,
            Self::Cancel => true,
        }
    }

    fn action(&self, _ctx: &Self::Context) -> OobAction {
        match self {
            Self::ApplyDb => OobAction::ApplyDb,
            Self::AssimilateDisk => OobAction::AssimilateDisk,
            Self::Acknowledge => OobAction::Acknowledge,
            Self::Cancel => OobAction::Cancel,
        }
    }

    fn protocol_binding(&self, _ctx: &Self::Context) -> ProtocolBinding {
        match self {
            Self::ApplyDb => ProtocolBinding::Transaction {
                decision_key: DecisionKey::OobResolution { bucket: ConflictBucket::Conflict },
                label: "Apply DB tags \u{2192} files".into(),
            },
            Self::AssimilateDisk => ProtocolBinding::Transaction {
                decision_key: DecisionKey::OobResolution { bucket: ConflictBucket::Conflict },
                label: "Assimilate file tags \u{2192} DB".into(),
            },
            Self::Acknowledge => ProtocolBinding::Transaction {
                decision_key: DecisionKey::OobResolution { bucket: ConflictBucket::MtimeOnly },
                label: "Acknowledge mtime changes".into(),
            },
            Self::Cancel => ProtocolBinding::Navigation,
        }
    }
}

// ============================================================================
// Action enum
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OobAction {
    None,
    Navigate,
    ApplyDb,
    AssimilateDisk,
    Acknowledge,
    Cancel,
}

// ============================================================================
// Per-bucket file state
// ============================================================================

pub struct OobBucketState {
    pub files: Vec<OobFile>,
    pub list: StandardListState,
}

impl OobBucketState {
    pub fn new(files: Vec<OobFile>) -> Self {
        let file_count = files.len();
        let mut list = StandardListState::new(StandardListConfig {
            multi_select: true,
            ..Default::default()
        });
        for i in 0..file_count {
            list.selected.insert(i);
        }

        Self { files, list }
    }

    pub fn current_file(&self) -> Option<&OobFile> {
        self.files.get(self.list.cursor)
    }

    fn navigate_up(&mut self) -> bool {
        if self.list.cursor > 0 {
            self.list.cursor -= 1;
            true
        } else {
            false
        }
    }

    fn navigate_down(&mut self) -> bool {
        if self.list.cursor + 1 < self.files.len() {
            self.list.cursor += 1;
            true
        } else {
            false
        }
    }

    fn page_up(&mut self) -> bool {
        let old = self.list.cursor;
        self.list.cursor = self.list.cursor.saturating_sub(20);
        self.list.cursor != old
    }

    fn page_down(&mut self) -> bool {
        let old = self.list.cursor;
        self.list.cursor = (self.list.cursor + 20).min(self.files.len().saturating_sub(1));
        self.list.cursor != old
    }
}

// ============================================================================
// OOB Resolution State
// ============================================================================

pub struct OobResolutionState {
    pub active_bucket: ConflictBucket,
    pub buckets: [OobBucketState; 4],
    pub bucket_counts: [usize; 4],
    pub frame: FrameState<OobButton>,
}

impl OobResolutionState {
    pub fn new(files: Vec<OobFile>) -> Self {
        let mut b0 = Vec::new();
        let mut b1 = Vec::new();
        let mut b2 = Vec::new();
        let mut b3 = Vec::new();

        for file in files {
            match file.bucket {
                ConflictBucket::MtimeOnly => b0.push(file),
                ConflictBucket::DbOnly => b1.push(file),
                ConflictBucket::DiskOnly => b2.push(file),
                ConflictBucket::Conflict => b3.push(file),
            }
        }

        let bucket_counts = [b0.len(), b1.len(), b2.len(), b3.len()];

        let initial_bucket = ConflictBucket::ALL
            .iter()
            .find(|b| bucket_counts[b.index()] > 0)
            .copied()
            .unwrap_or(ConflictBucket::MtimeOnly);

        let buckets = [
            OobBucketState::new(b0),
            OobBucketState::new(b1),
            OobBucketState::new(b2),
            OobBucketState::new(b3),
        ];

        let mut frame = FrameState::new();
        if initial_bucket.is_acknowledgeable() {
            frame.buttons.selected = OobButton::Acknowledge;
        }

        Self {
            active_bucket: initial_bucket,
            buckets,
            bucket_counts,
            frame,
        }
    }

    pub fn selected_path(&self) -> Option<&str> {
        self.active_bucket_state()
            .current_file()
            .map(|f| f.path.as_str())
    }

    pub fn active_bucket_state(&self) -> &OobBucketState {
        &self.buckets[self.active_bucket.index()]
    }

    pub fn active_bucket_state_mut(&mut self) -> &mut OobBucketState {
        &mut self.buckets[self.active_bucket.index()]
    }

    pub fn total_files(&self) -> usize {
        self.bucket_counts.iter().sum()
    }

    pub fn button_ctx(&self) -> OobButtonCtx {
        OobButtonCtx {
            is_resolvable: self.active_bucket.is_resolvable(),
            is_acknowledgeable: self.active_bucket.is_acknowledgeable(),
            has_files: !self.active_bucket_state().files.is_empty(),
        }
    }

    pub fn current_mismatches(&self) -> &[TagMismatchEntry] {
        self.active_bucket_state()
            .current_file()
            .map(|f| f.mismatches.as_slice())
            .unwrap_or(&[])
    }

    pub fn handle_click(&mut self, x: u16, y: u16) -> Option<OobAction> {
        let ctx = self.button_ctx();
        if let Some(action) = self.frame.buttons.handle_click(x, y, &ctx) {
            self.frame.focus_pane = FocusPane::Buttons;
            Some(action)
        } else {
            None
        }
    }

    pub fn handle_input(&mut self, action: &InputAction) -> OobAction {
        match action {
            // Select all / deselect all
            InputAction::TextHome => {
                let bucket = self.active_bucket_state_mut();
                let all_count = bucket.files.len();
                let all_selected = bucket.list.selected.len() == all_count;
                if all_selected {
                    bucket.list.selected.clear();
                } else {
                    for i in 0..all_count { bucket.list.selected.insert(i); }
                }
                return OobAction::None;
            }
            InputAction::Toggle => {
                let bucket = self.active_bucket_state_mut();
                if !bucket.files.is_empty() {
                    let cursor = bucket.list.cursor;
                    if bucket.list.selected.contains(&cursor) {
                        bucket.list.selected.remove(&cursor);
                    } else {
                        bucket.list.selected.insert(cursor);
                    }
                }
                return OobAction::None;
            }
            InputAction::CycleNext => {
                self.active_bucket = self.active_bucket.next();
                let ctx = self.button_ctx();
                self.frame.buttons.clamp(&ctx);
                return OobAction::Navigate;
            }
            InputAction::CyclePrev => {
                self.active_bucket = self.active_bucket.prev();
                let ctx = self.button_ctx();
                self.frame.buttons.clamp(&ctx);
                return OobAction::Navigate;
            }
            InputAction::NavUp => {
                return if self.active_bucket_state_mut().navigate_up() {
                    OobAction::Navigate
                } else { OobAction::None };
            }
            InputAction::NavDown => {
                return if self.active_bucket_state_mut().navigate_down() {
                    OobAction::Navigate
                } else { OobAction::None };
            }
            InputAction::PageUp => {
                return if self.active_bucket_state_mut().page_up() {
                    OobAction::Navigate
                } else { OobAction::None };
            }
            InputAction::PageDown => {
                return if self.active_bucket_state_mut().page_down() {
                    OobAction::Navigate
                } else { OobAction::None };
            }
            _ => {}
        }

        match self.handle_frame_input(action) {
            FrameInputResult::Action(a) => a,
            _ => OobAction::None,
        }
    }
}

// ============================================================================
// Dispatchable
// ============================================================================

impl super::dispatch::Dispatchable for OobResolutionState {
    type Action = OobAction;

    fn dispatch(
        &self,
        action: OobAction,
        resolver: &mm_meta::paths::PathResolver,
    ) -> super::dispatch::DispatchResult {
        use mm_meta::db_types::Zone;
        use mm_meta::mutations::indexing::{
            AcknowledgeMtimeOnlyMutation, ApplyDbTagsToDiskMutation,
            AssimilateDiskTagsToDbMutation,
        };
        use mm_meta::mutations::Mutation;
        use super::dispatch::DispatchResult;

        match action {
            OobAction::None | OobAction::Navigate => DispatchResult::Handled,
            OobAction::ApplyDb | OobAction::AssimilateDisk => {
                let bucket_state = self.active_bucket_state();
                let indices: Vec<usize> = if !bucket_state.list.selected.is_empty() {
                    bucket_state.list.selected.iter().copied().collect()
                } else {
                    (0..bucket_state.files.len()).collect()
                };

                let tracks: Vec<(i64, std::path::PathBuf)> = indices
                    .iter()
                    .filter_map(|&idx| bucket_state.files.get(idx))
                    .map(|f| (f.inode, resolver.resolve_for_zone(Zone::Corpus, std::path::Path::new(&f.path))))
                    .collect();

                if tracks.is_empty() {
                    return DispatchResult::Handled;
                }

                let (label, mutations): (&str, Vec<Mutation>) = if action == OobAction::ApplyDb {
                    (
                        "Apply DB tags \u{2192} files",
                        tracks
                            .into_iter()
                            .map(|(inode, path)| {
                                Mutation::ApplyDbTagsToDisk(ApplyDbTagsToDiskMutation {
                                    inode,
                                    path,
                                    zone: Zone::Corpus,
                                })
                            })
                            .collect(),
                    )
                } else {
                    (
                        "Assimilate file tags \u{2192} DB",
                        tracks
                            .into_iter()
                            .map(|(inode, path)| {
                                Mutation::AssimilateDiskTagsToDb(AssimilateDiskTagsToDbMutation {
                                    inode,
                                    path,
                                    zone: None,
                                })
                            })
                            .collect(),
                    )
                };

                let key = DecisionKey::OobResolution {
                    bucket: self.active_bucket,
                };

                DispatchResult::Stage {
                    key,
                    label: label.into(),
                    mutations,
                }
            }
            OobAction::Acknowledge => {
                let bucket_state = self.active_bucket_state();
                let indices: Vec<usize> = if !bucket_state.list.selected.is_empty() {
                    bucket_state.list.selected.iter().copied().collect()
                } else {
                    (0..bucket_state.files.len()).collect()
                };

                let tracks: Vec<(i64, std::path::PathBuf)> = indices
                    .iter()
                    .filter_map(|&idx| bucket_state.files.get(idx))
                    .map(|f| (f.inode, resolver.resolve_for_zone(Zone::Corpus, std::path::Path::new(&f.path))))
                    .collect();

                if tracks.is_empty() {
                    return DispatchResult::Handled;
                }

                let mutations = vec![Mutation::AcknowledgeMtimeOnly(
                    AcknowledgeMtimeOnlyMutation { tracks },
                )];

                DispatchResult::Stage {
                    key: DecisionKey::OobResolution {
                        bucket: ConflictBucket::MtimeOnly,
                    },
                    label: "Acknowledge mtime changes".into(),
                    mutations,
                }
            }
            OobAction::Cancel => DispatchResult::Cancel,
        }
    }

    fn cancel_message(&self) -> &'static str {
        "OOB resolution closed"
    }
}

impl ModalFrameCore for OobResolutionState {
    type Button = OobButton;

    fn content_layout(&self) -> ContentLayout {
        ContentLayout::HorizontalSplit { list_percent: 33, info_height: 4 }
    }

    fn list_title(&self) -> String { "Files".into() }

    fn frame_state(&self) -> &FrameState<OobButton> { &self.frame }
    fn frame_state_mut(&mut self) -> &mut FrameState<OobButton> { &mut self.frame }
    fn cursor(&self) -> usize { self.active_bucket_state().list.cursor }
    fn cursor_mut(&mut self) -> &mut usize { &mut self.active_bucket_state_mut().list.cursor }
    fn list_len(&self) -> usize { self.active_bucket_state().files.len() }
    fn button_ctx(&self) -> OobButtonCtx { OobResolutionState::button_ctx(self) }
    fn escape_action(&self) -> OobAction { OobAction::Cancel }
}
